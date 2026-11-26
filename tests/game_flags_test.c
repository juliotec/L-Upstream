/* The game's flags (game/flags.c) over a GameDataMgr made up here, in two bounded windows of guest memory (the
   singleton slots and a heap; every address outside them reads as unmapped). tests/test_game_flags.py builds it with a
   hos.h of its own (game/../hos.h) and runs it. No flag name, value or layout is taken from a dump: the layout is the
   one flags.c declares, the names are made up or the few the host names in its source. */
#include "hos.h"
#include "game/flags.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)

unsigned char slots[0x100000], heap[0x100000];
int marks;
void (*range_hook)(uint64_t a, uint64_t n);

static void *at(uint64_t a, uint64_t n) { void *p = hos_guest_range(a, n); CHECK(p); return p; }
static void w64(uint64_t a, uint64_t v) { memcpy(at(a, 8), &v, 8); }
static void w32(uint64_t a, uint32_t v) { memcpy(at(a, 4), &v, 4); }
static void w8(uint64_t a, uint8_t v) { memcpy(at(a, 1), &v, 1); }
static uint8_t r8(uint64_t a) { return *(uint8_t *)at(a, 1); }
static uint64_t r64(uint64_t a) { uint64_t v; memcpy(&v, at(a, 8), 8); return v; }
static uint32_t r32(uint64_t a) { uint32_t v; memcpy(&v, at(a, 4), 4); return v; }
static uint64_t heap_next = HEAP + 0x1000;
static uint64_t alloc(uint64_t n) {
    const uint64_t a = heap_next;
    heap_next += (n + 15) & ~15ull;
    CHECK(heap_next < HEAP + sizeof heap);
    return a;
}
static uint64_t text(const char *s) {
    const uint64_t a = alloc(64);
    memcpy(at(a, strlen(s) + 1), s, strlen(s) + 1);
    return a;
}
/* a slot rewritten between game_flag_slot's two reads of it: at its second read, its entry made another */
static uint64_t torn_at;
static int torn_reads;
static void torn_hook(uint64_t a, uint64_t n) {
    if (a != torn_at || n != 8 || ++torn_reads != 2) return;
    range_hook = NULL;
    w32(a + 4, r32(a + 4) ? 0 : 1);
    range_hook = torn_hook;
}
static uint32_t H(const char *s) { return game_flag_hash(s, strlen(s)); }

/* a container of size slots in mgr, and a flag in it: the address of its value */
static void container(uint64_t mgr, int kind, uint32_t size) {
    const uint64_t b = mgr + game_flag_layout[kind].at;
    w64(b + 16, alloc((uint64_t)game_flag_layout[kind].stride * size));
    w64(b + 24, alloc(8ull * size));
    w32(b + 36, size);
    w32(b + 8, 0);
}
static uint64_t flag(uint64_t mgr, int kind, uint32_t hash) {
    const uint64_t b = mgr + game_flag_layout[kind].at, table = r64(b + 24);
    const uint32_t size = r32(b + 36), count = r32(b + 8);
    CHECK(hash && count < size);
    uint32_t slot = hash % size;
    while (r32(table + slot * 8ull)) slot = (slot + 1) % size;
    w32(table + slot * 8ull, hash);
    w32(table + slot * 8ull + 4, count);
    w32(b + 8, count + 1);
    return r64(b + 16) + (uint64_t)count * game_flag_layout[kind].stride + game_flag_layout[kind].value;
}

static void hash_case(void) {
    /* (MurmurHash3 x86 32, seed 0: the reference vectors, and names the host's source gives with their hashes) */
    CHECK(game_flag_hash("", 0) == 0);
    CHECK(H("a") == 0x3c2569b2u && H("ab") == 0x9bbfd75fu && H("abc") == 0xb3dd93fau);
    CHECK(H("PlayerStatus.Life") == 0x31ab5580u);
    CHECK(H("OptionData.VoiceLanguage") == 0xa2dcdc9du);
    CHECK(H("Sequence_CurrentBanc") == 0x1d6189dau && H("Sequence_IsChildhood") == 0xcea220d3u);
    CHECK(H("Sequence_IsLoadingBanc") == 0xef2b60c2u && H("MapIconDeactivateByAreaActor") == 0x9a4dd779u);
    CHECK(H("IsGet.Item_Melody_ZeldaLullaby") == 0x01c20002u);
    puts("PASS: the flags' hash is MurmurHash3 x86 32 with seed 0");
}

static void read_case(void) {
    CHECK(game_flags_mgr() == 0);   /* (no GameDataMgr yet) */
    const uint64_t mgr = alloc(0x1000), holder = alloc(16);
    w64(GAME_FLAGS_SLOT, holder), w64(holder, mgr);
    CHECK(game_flags_mgr() == mgr);
    for (int k = 0; k < GAME_FLAG_KINDS; k++) container(mgr, k, 16);
    int b;
    int64_t v;
    float f;
    char t[80];
    /* Bool: the byte as it is for a number, its low bit for a bool */
    const uint64_t child = flag(mgr, GAME_FLAG_BOOL, H("Sequence_IsChildhood"));
    w8(child, 1);
    CHECK(game_flag_bool(mgr, H("Sequence_IsChildhood"), &b) && b == 1);
    w8(child, 2);
    CHECK(game_flag_number(mgr, GAME_FLAG_BOOL, H("Sequence_IsChildhood"), &v) && v == 2);
    CHECK(game_flag_bool(mgr, H("Sequence_IsChildhood"), &b) && b == 0);
    /* collisions: three hashes on one slot, walked on past the table's end (15, then 0 and 1) */
    const uint32_t c1 = 15, c2 = 15 + 16, c3 = 15 + 32;
    w8(flag(mgr, GAME_FLAG_BOOL, c1), 1), w8(flag(mgr, GAME_FLAG_BOOL, c2), 0), w8(flag(mgr, GAME_FLAG_BOOL, c3), 1);
    CHECK(game_flag_bool(mgr, c1, &b) && b == 1 && game_flag_bool(mgr, c2, &b) && b == 0 && game_flag_bool(mgr, c3, &b) && b == 1);
    CHECK(!game_flag_bool(mgr, 15 + 48, &b));   /* (the walk ends at an empty slot) */
    CHECK(!game_flag_bool(mgr, 0, &b));          /* (0 is no flag's hash) */
    CHECK(!game_flag_number(mgr, GAME_FLAG_INT, H("Sequence_IsChildhood"), &v));   /* (another container) */
    /* Int signed, UInt and Enum unsigned */
    w32(flag(mgr, GAME_FLAG_INT, H("Test.Negative")), (uint32_t)-3);
    CHECK(game_flag_number(mgr, GAME_FLAG_INT, H("Test.Negative"), &v) && v == -3);
    w32(flag(mgr, GAME_FLAG_UINT, H("Test.Unsigned")), 3000000000u);
    CHECK(game_flag_number(mgr, GAME_FLAG_UINT, H("Test.Unsigned"), &v) && v == 3000000000ll);
    w32(flag(mgr, GAME_FLAG_ENUM, H("Test.Enum")), 0xdeadbeefu);
    CHECK(game_flag_number(mgr, GAME_FLAG_ENUM, H("Test.Enum"), &v) && v == 0xdeadbeefll);
    CHECK(!game_flag_number(mgr, GAME_FLAG_FLOAT, H("Test.Enum"), &v) && !game_flag_number(mgr, GAME_FLAG_TEXT, H("Test.Enum"), &v));
    /* Float: finite only */
    const uint64_t rot = flag(mgr, GAME_FLAG_FLOAT, H("Test.Float"));
    const float minus = -71.5f, nan = NAN;
    memcpy(at(rot, 4), &minus, 4);
    CHECK(game_flag_float(mgr, H("Test.Float"), &f) && f == -71.5f);
    memcpy(at(rot, 4), &nan, 4);
    CHECK(!game_flag_float(mgr, H("Test.Float"), &f));
    /* String64: through its pointer, at most 64 bytes, always ended; an unreadable pointer is none, with "" */
    const uint64_t map = flag(mgr, GAME_FLAG_TEXT, H("Sequence_CurrentBanc"));
    w64(map, text("KokiriForest_Child"));
    CHECK(game_flag_text(mgr, H("Sequence_CurrentBanc"), t, sizeof t) && !strcmp(t, "KokiriForest_Child"));
    CHECK(game_flag_text(mgr, H("Sequence_CurrentBanc"), t, 8) && !strcmp(t, "KokiriF"));
    char long_text[70];
    memset(long_text, 'x', 69), long_text[69] = 0;
    const uint64_t far = alloc(80);
    memcpy(at(far, 70), long_text, 70);
    w64(map, far);
    CHECK(game_flag_text(mgr, H("Sequence_CurrentBanc"), t, sizeof t) && strlen(t) == 63);
    w64(map, 0x7000000000ull);
    CHECK(!game_flag_text(mgr, H("Sequence_CurrentBanc"), t, sizeof t) && !t[0]);
    w64(map, HEAP + sizeof heap - 8);   /* (64 bytes would run past the window) */
    CHECK(!game_flag_text(mgr, H("Sequence_CurrentBanc"), t, sizeof t) && !t[0]);
    CHECK(!game_flag_text(mgr, H("Sequence_CurrentBanc"), t, 0));
    /* an entry past the count, a size past the cap, a table out of the window, no GameDataMgr: no flag, no fault */
    const uint64_t box = mgr + game_flag_layout[GAME_FLAG_INT].at;
    const uint32_t count = r32(box + 8);
    w32(box + 8, 0);
    CHECK(!game_flag_number(mgr, GAME_FLAG_INT, H("Test.Negative"), &v));
    w32(box + 8, count);
    w32(box + 36, (1u << 22) + 1);
    CHECK(!game_flag_number(mgr, GAME_FLAG_INT, H("Test.Negative"), &v));
    w32(box + 36, 16);
    const uint64_t table = r64(box + 24);
    w64(box + 24, 0x7000000000ull);
    CHECK(!game_flag_number(mgr, GAME_FLAG_INT, H("Test.Negative"), &v));
    w64(box + 24, table);
    CHECK(game_flag_number(mgr, GAME_FLAG_INT, H("Test.Negative"), &v) && v == -3);
    CHECK(!game_flag_number(0, GAME_FLAG_INT, H("Test.Negative"), &v) && !game_flag_entry(mgr, GAME_FLAG_KINDS, 1) &&
          !game_flag_entry(mgr, -1, 1));
    puts("PASS: Bool, Int, UInt, Enum, Float and String64 flags read by hash, collisions walked; anything out of bounds is none");

    /* the write: the live byte, marked written; none for a flag that is not there */
    marks = 0;
    CHECK(game_flag_set_bool(mgr, H("Sequence_IsChildhood"), 7) && r8(child) == 1 && marks == 1);
    CHECK(game_flag_set_bool(mgr, H("Sequence_IsChildhood"), 0) && r8(child) == 0 && marks == 2);
    CHECK(!game_flag_set_bool(mgr, H("No.Such"), 1) && marks == 2);
    puts("PASS: a Bool written is its live byte, 0 or 1, marked written for the GPU's mirror; a missing flag is not written");

    /* Bool64bitKey: by a 64-bit id, the place it was found kept */
    const uint64_t list = alloc(16 * 3);
    w32(mgr + 0xc58, 3), w64(mgr + 0xc58 + 8, list);
    for (int i = 0; i < 3; i++) w64(list + 16 * i, 0x1111000000000000ull + (uint64_t)i), w8(list + 16 * i + 12, (uint8_t)(i & 1));
    int found = -1;
    CHECK(game_flag_bool64(mgr, 0x1111000000000001ull, &found) == 1 && found == 1);
    CHECK(game_flag_bool64(mgr, 0x1111000000000001ull, &found) == 1 && found == 1);
    found = -1;
    CHECK(game_flag_bool64(mgr, 0x1111000000000002ull, &found) == 0 && found == 2);
    found = 2;   /* (a stale place: searched again) */
    CHECK(game_flag_bool64(mgr, 0x1111000000000001ull, &found) == 1 && found == 1);
    CHECK(game_flag_bool64(mgr, 0x2222ull, &found) == -1);
    w32(mgr + 0xc58, (1u << 20) + 1);
    CHECK(game_flag_bool64(mgr, 0x1111000000000001ull, &found) == -1);
    CHECK(game_flag_bool64(0, 1, &found) == -1);
    w32(mgr + 0xc58, 3);
    puts("PASS: a Bool64bitKey flag by its id: 1, 0, or -1 when not known; the place kept, a stale one searched again");

    /* a table's header and its slots, as the changes scan reads them */
    GameFlagTable h;
    CHECK(game_flag_table(mgr, GAME_FLAG_BOOL, &h) && h.size == 16 && h.count == 4);
    uint32_t key, value, flags_seen = 0;
    for (uint32_t s = 0; s < h.size; s++) {
        const int got = game_flag_slot(&h, GAME_FLAG_BOOL, s, &key, &value);
        CHECK(got >= 0);
        if (!got) CHECK(!key && !value);
        else {
            flags_seen++;
            CHECK(key && game_flag_bool(mgr, key, &b) && (uint32_t)b == (value & 1));
        }
    }
    CHECK(flags_seen == 4);
    CHECK(game_flag_slot(&h, GAME_FLAG_BOOL, 16, &key, &value) == -1);   /* (past the size) */
    {   /* (a slot rewritten while it is read: the pair read again differs, so the value may be another entry's) */
        uint32_t s = 0;
        while (s < h.size && game_flag_slot(&h, GAME_FLAG_BOOL, s, &key, &value) != 1) s++;
        CHECK(s < h.size);
        const uint32_t entry = r32(h.table + s * 8ull + 4);
        torn_at = h.table + s * 8ull, torn_reads = 0, range_hook = torn_hook;
        CHECK(game_flag_slot(&h, GAME_FLAG_BOOL, s, &key, &value) == -1 && !key && !value);
        range_hook = NULL;
        CHECK(torn_reads == 2);
        w32(h.table + s * 8ull + 4, entry);
        CHECK(game_flag_slot(&h, GAME_FLAG_BOOL, s, &key, &value) == 1);
    }
    w32(h.table + 15 * 8 + 4, 99);   /* (an entry past the count) */
    CHECK(game_flag_slot(&h, GAME_FLAG_BOOL, 15, &key, &value) == -1);
    const uint64_t bbox = mgr + game_flag_layout[GAME_FLAG_BOOL].at;
    w32(bbox + 8, 17);   /* (more entries than slots) */
    CHECK(!game_flag_table(mgr, GAME_FLAG_BOOL, &h));
    w32(bbox + 8, 4);
    w32(bbox + 36, 1u << 22);   /* (within the cap, but the table is not that big in the window) */
    CHECK(!game_flag_table(mgr, GAME_FLAG_BOOL, &h));
    CHECK(!game_flag_table(0, GAME_FLAG_BOOL, &h));
    puts("PASS: a container's header is checked whole; its slots read as a flag, empty, or unreadable");
}

int main(void) {
    hash_case();
    read_case();
    return 0;
}
