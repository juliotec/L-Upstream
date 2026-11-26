/* host/zs.c's streamed read of a .zs file the .nup wraps in a zstd frame of its own: unwrapped as it streams (nothing of
   it decoded whole in memory), the same bytes as the whole read. Built and run by builder/crates/game-files (romfs.rs,
   the_host_streams_a_wrapped_zs) with host/romfs.c and the vendored zstd decoder, on the synthetic .nup's index: one
   whose .nup frame has a window past 128 MB too, a frame fed one byte first (its header whole only later), and a frame
   that does not say its dictionary's id, read with the one its reader names (hos_zs_decode_named).

       zs_test <index> <dictionary> <a frame made with it> */
#include "../host/romfs.h"
#include "../host/romfs_index.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../host/zs.c"   /* (as it is: its static zs_feed fed here too) */

const char *hos_romfs_dir = "/no/such/romfs";
HosRomfsIndex *hos_romfs_index;
void hos_log(const char *fmt, ...);
void hos_log(const char *fmt, ...) { (void)fmt; }
int MoveFileExA(const char *from, const char *to, unsigned flags);
int MoveFileExA(const char *from, const char *to, unsigned flags) { (void)from, (void)to, (void)flags; return 0; }   /* (hos_zs_write's: not called) */

typedef struct { uint8_t *b; size_t n; uint64_t most_decoded; } Got;
static int take(void *arg, const uint8_t *piece, size_t n) {
    Got *g = (Got *)arg;
    g->b = (uint8_t *)realloc(g->b, g->n + n);
    memcpy(g->b + g->n, piece, n);
    g->n += n;
    const uint64_t now = hos_romfs_decoded_bytes();
    if (now > g->most_decoded) g->most_decoded = now;
    return 1;
}

static uint8_t *slurp(const char *path, size_t *n) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    *n = (size_t)ftell(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *b = (uint8_t *)malloc(*n + 1);
    if (b && fread(b, 1, *n, f) != *n) free(b), b = NULL;
    fclose(f);
    return b;
}

/* a frame made with a dictionary, fed one byte first and then the rest: its dictionary still found by its id */
static int one_byte_first(const char *dict_path, const char *frame_path) {
    size_t dn = 0, fn = 0;
    uint8_t *dict = slurp(dict_path, &dn), *frame = slurp(frame_path, &fn);
    if (!dict || !frame || fn < 2) return 0;
    dicts[0] = (Dict){ZSTD_getDictID_fromDict(dict, dn), ZSTD_createDDict(dict, dn), {0}};   /* (as read_dicts would) */
    ndicts = 1;
    Got g = {NULL, 0, 0};
    ZsStream z = {NULL, (uint8_t *)malloc(ZSTD_DStreamOutSize()), ZSTD_DStreamOutSize(), 1, 1, take, &g, {0}, 0};
    zs_feed(&z, frame, 1);
    zs_feed(&z, frame + 1, fn - 1);
    if (z.ok && z.last) zs_feed(&z, frame, 0);
    const unsigned long long want = ZSTD_getFrameContentSize(frame, fn);
    uint8_t *plain = (uint8_t *)malloc((size_t)want + 1);
    ZSTD_DCtx *ctx = ZSTD_createDCtx();
    const size_t got = ZSTD_decompress_usingDDict(ctx, plain, (size_t)want, frame, fn, dicts[0].dict);
    const int ok = z.ok && !z.last && got == want && g.n == want && !memcmp(plain, g.b, g.n);
    if (z.ctx) ZSTD_freeDCtx(z.ctx);
    ZSTD_freeDCtx(ctx);
    free(z.out), free(plain), free(g.b), free(dict), free(frame);
    return ok;
}

/* a SARC of these files (names, contents), as the game's Pack/ZsDic.pack.zs holds its dictionaries: malloc'd, *n bytes */
static uint8_t *sarc(const char *const *names, const uint8_t *const *files, const size_t *sizes, int count, size_t *n) {
    size_t names_n = 0, data_n = 0;
    for (int i = 0; i < count; i++) names_n += (strlen(names[i]) + 4) & ~(size_t)3, data_n += (sizes[i] + 3) & ~(size_t)3;
    const size_t nodes = 0x20, names_at = nodes + 16 * (size_t)count + 8, data = names_at + names_n;
    uint8_t *p = (uint8_t *)calloc(1, data + data_n);
    memcpy(p, "SARC", 4), p[4] = 0x14, p[6] = 0xff, p[7] = 0xfe;
    for (int k = 0; k < 4; k++) p[12 + k] = (uint8_t)(data >> 8 * k);
    memcpy(p + 0x14, "SFAT", 4), p[0x18] = 0x0c, p[0x1a] = (uint8_t)count;
    memcpy(p + names_at - 8, "SFNT", 4), p[names_at - 4] = 8;
    size_t name_at = 0, file_at = 0;
    for (int i = 0; i < count; i++) {
        uint8_t *e = p + nodes + 16 * (size_t)i;
        const uint32_t attr = 0x01000000u | (uint32_t)(name_at / 4), lo = (uint32_t)file_at, hi = (uint32_t)(file_at + sizes[i]);
        for (int k = 0; k < 4; k++) e[4 + k] = (uint8_t)(attr >> 8 * k), e[8 + k] = (uint8_t)(lo >> 8 * k), e[12 + k] = (uint8_t)(hi >> 8 * k);
        strcpy((char *)p + names_at + name_at, names[i]);
        memcpy(p + data + file_at, files[i], sizes[i]);
        name_at += (strlen(names[i]) + 4) & ~(size_t)3, file_at += (sizes[i] + 3) & ~(size_t)3;
    }
    *n = data + data_n;
    return p;
}

/* a frame made with the dictionary that does not say its id (zstd leaves it out when asked): not read plainly, read with
   the dictionary its reader names, found in the SARC by its name (hos_zs_decode_named: the sound archives, the maps) */
static int named_fallback(const char *dict_path, const char *frame_path) {
    size_t dn = 0, fn = 0;
    uint8_t *dict = slurp(dict_path, &dn), *frame = slurp(frame_path, &fn);
    if (!dict || !frame || fn < 6 || !ZSTD_getDictID_fromFrame(frame, fn)) return 0;
    /* (the id cut out of the header: its descriptor's two low bits say its size, after the window byte unless the
       frame is a single segment) */
    static const size_t did_sizes[4] = {0, 1, 2, 4};
    const uint8_t fhd = frame[4];
    const size_t did_at = 5 + !(fhd & 0x20), did_n = did_sizes[fhd & 3];
    uint8_t *cut = (uint8_t *)malloc(fn);
    memcpy(cut, frame, did_at);
    cut[4] = (uint8_t)(fhd & ~3u);
    memcpy(cut + did_at, frame + did_at + did_n, fn - did_at - did_n);
    const size_t cut_n = fn - did_n;
    const char *names[] = {"pack.zsdic", "zs.zsdic"};
    const uint8_t *files[] = {(const uint8_t *)"not a dictionary of these", dict};
    const size_t sizes[] = {25, dn};
    size_t sn = 0;
    uint8_t *s = sarc(names, files, sizes, 2, &sn);
    for (int i = 0; i < ndicts; i++) ZSTD_freeDDict(dicts[i].dict);
    ndicts = 0;
    const int parsed = dicts_from_sarc(s, sn) && ndicts == 2 && !strcmp(dicts[0].name, "pack.zsdic") &&
                       !strcmp(dicts[1].name, "zs.zsdic") && dicts[1].id == ZSTD_getDictID_fromDict(dict, dn);
    size_t want_n = 0, got_n = 0, none_n = 0;
    uint8_t *want = hos_zs_decode(frame, fn, &want_n, 1u << 20), *plain = hos_zs_decode(cut, cut_n, &none_n, 1u << 20),
            *got = hos_zs_decode_named(cut, cut_n, &got_n, 1u << 20, "zs.zsdic"),
            *other = hos_zs_decode_named(cut, cut_n, &none_n, 1u << 20, "pack.zsdic");
    const int ok = parsed && want && !plain && !other && got && got_n == want_n && !memcmp(got, want, want_n);
    if (!ok) fprintf(stderr, "named: parsed %d, by id %d, plain %d, named %d, other %d\n", parsed, !!want, !!plain, !!got, !!other);
    free(want), free(plain), free(got), free(other), free(s), free(cut), free(dict), free(frame);
    return ok;
}

int main(int argc, char **argv) {
    char why[512];
    if (argc != 4 || !(hos_romfs_index = hos_romfs_index_open(argv[1], why, sizeof why))) return 2;
    if (!one_byte_first(argv[2], argv[3])) { fprintf(stderr, "FAIL a frame fed one byte first\n"); return 1; }
    if (!named_fallback(argv[2], argv[3])) { fprintf(stderr, "FAIL a frame of no id, by its dictionary's name\n"); return 1; }
    Got w = {NULL, 0, 0};   /* (the .nup's frame round it asks for a window past 128 MB) */
    if (!hos_zs_romfs_stream("Pack/WideWrapped.zs", take, &w)) { fprintf(stderr, "FAIL streamed, a wide window round it\n"); return 1; }
    free(w.b);
    const uint64_t before = hos_romfs_decoded_bytes();   /* (the dictionaries' pack, decoded and kept: romfs.c) */
    Got g = {NULL, 0, before};
    if (!hos_zs_romfs_stream("Pack/Wrapped.zs", take, &g)) { fprintf(stderr, "FAIL streamed\n"); return 1; }
    if (g.most_decoded > before) { fprintf(stderr, "FAIL the wrapped file decoded whole (%llu bytes)\n", (unsigned long long)g.most_decoded); return 1; }
    /* (the whole read: the .nup's frame unwrapped by romfs.c, the file's own frame decoded here) */
    size_t zn = 0, n = 0;
    uint8_t *z = hos_romfs_read("Pack/Wrapped.zs", &zn, 1u << 30), *whole = (uint8_t *)malloc(g.n + 1);
    ZSTD_DCtx *ctx = ZSTD_createDCtx();
    ZSTD_inBuffer ib = {z, zn, 0};
    ZSTD_outBuffer ob = {whole, g.n + 1, 0};
    const size_t r = z ? ZSTD_decompressStream(ctx, &ob, &ib) : 1;
    n = ob.pos;
    if (r || n != g.n || memcmp(whole, g.b, n)) { fprintf(stderr, "FAIL not the whole read's bytes\n"); return 1; }
    printf("ok %zu bytes\n", n);
    return 0;
}
