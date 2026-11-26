/* (hookshot) The game's .zs files, for the host's own reads: a zstd frame, made with one of the game's dictionaries or
   with none. The dictionaries are the members of Pack/ZsDic.pack.zs (a SARC archive in a zstd frame of its own, made
   without one), read from the RomFS (romfs.h) once, the first time a frame asks for one (hos_zs_dict, by its id).
   Every zstd read of the host's own goes through here: the resource size tables (game/mods.c, host/prompts.c), the
   sound archives (host/audio_source.c), the heart containers' maps (game/oot_heart_containers.c), the prompts' game
   files as they stream (hos_zs_romfs_stream), and romfs.c's zstd-stored files of the .nup (hos_zs_frame,
   hos_zs_frame_head). zstd's decoder is third_party/zstd (BSD), built into the executable (builder/crates/build-compile)
   as into the Vulkan back end. hos_zs_write writes a frame of raw blocks (prompts_base.h's writer): the game's decoder
   takes a frame made without its dictionary. */
#ifndef HOS_ZS_ALONE   /* (tests/zs_test.c builds this file with romfs.c alone) */
#include "hos.h"
#else
#include <stdint.h>
void hos_log(const char *fmt, ...);
#endif
#include "romfs.h"
#include "prompts_base.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#define ZSTD_STATIC_LINKING_ONLY
#include "zstd.h"


typedef struct { unsigned id; ZSTD_DDict *dict; char name[48]; } Dict;   /* (id 0: a dictionary of raw content, found by its name only) */
static Dict dicts[16];
static int ndicts;

static uint8_t *read_file(const char *path, size_t *n) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    const long long size = _ftelli64(f);
    fseek(f, 0, SEEK_SET);
    uint8_t *b = size >= 0 && size < (1ll << 30) ? (uint8_t *)malloc((size_t)size + 1) : NULL;
    if (b && fread(b, 1, (size_t)size, f) != (size_t)size) free(b), b = NULL;
    fclose(f);
    if (b) *n = (size_t)size;
    return b;
}

static uint32_t rd16(const uint8_t *p) { return p[0] | p[1] << 8; }
static uint32_t rd32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

/* The plain decode of a frame (no dictionary): malloc'd, NULL when it is not one or it holds more than limit bytes */
static uint8_t *decode(const uint8_t *z, size_t zn, size_t *n, size_t limit, const ZSTD_DDict *dict) {
    const unsigned long long raw = ZSTD_getFrameContentSize(z, zn);
    if (raw == ZSTD_CONTENTSIZE_ERROR || raw == ZSTD_CONTENTSIZE_UNKNOWN || raw > limit) return NULL;
    uint8_t *d = (uint8_t *)malloc((size_t)raw + 1);
    ZSTD_DCtx *ctx = d ? ZSTD_createDCtx() : NULL;
    size_t got = 0;
    if (ctx) got = dict ? ZSTD_decompress_usingDDict(ctx, d, (size_t)raw, z, zn, dict) : ZSTD_decompressDCtx(ctx, d, (size_t)raw, z, zn);
    if (ctx) ZSTD_freeDCtx(ctx);
    if (!ctx || ZSTD_isError(got) || got != raw) {
        free(d);
        return NULL;
    }
    *n = (size_t)raw;
    return d;
}

/* the dictionaries of a SARC ("SARC", u16 header size, u16 BOM, u32 size, u32 data offset; SFAT after the header:
   "SFAT", u16 header size, u16 node count, u32 hash key, nodes of {u32 hash, u32 attributes, u32 start, u32 end}; then
   SFNT, its 8-byte header, and the names, each node's at 4 * the low 24 bits of its attributes, 0-terminated), with
   their ids and names. 0 when it is no SARC */
static int dicts_from_sarc(const uint8_t *p, size_t n) {
    if (n < 0x14 || memcmp(p, "SARC", 4) || rd16(p + 6) != 0xfeff) return 0;
    const size_t head = rd16(p + 4), data = rd32(p + 12);
    if (head + 12 > n || memcmp(p + head, "SFAT", 4) || data > n) return 0;
    const size_t count = rd16(p + head + 6), nodes = head + rd16(p + head + 4), names = nodes + count * 16 + 8;
    for (size_t i = 0; i < count && nodes + i * 16 + 16 <= n && ndicts < (int)(sizeof dicts / sizeof *dicts); i++) {
        const uint8_t *e = p + nodes + i * 16;
        const uint32_t lo = rd32(e + 8), hi = rd32(e + 12);
        if (lo > hi || hi > n - data) continue;
        ZSTD_DDict *d = ZSTD_createDDict(p + data + lo, hi - lo);
        if (!d) continue;
        Dict *k = &dicts[ndicts++];
        *k = (Dict){ZSTD_getDictID_fromDict(p + data + lo, hi - lo), d, {0}};
        const size_t at = names + 4 * (size_t)(rd32(e + 4) & 0xffffff);
        if (at < n && memchr(p + at, 0, n - at) && strlen((const char *)p + at) < sizeof k->name) strcpy(k->name, (const char *)p + at);
    }
    return 1;
}

/* the dictionaries, out of Pack/ZsDic.pack.zs (a SARC in a plain frame) */
static void read_dicts(void) {
    const char *path = "Pack/ZsDic.pack.zs";
    if (!hos_romfs_ready()) return;
    size_t zn = 0, n = 0;
    uint8_t *z = hos_romfs_read(path, &zn, 1u << 30), *p = z ? decode(z, zn, &n, 64u << 20, NULL) : NULL;
    free(z);
    if (!p || !dicts_from_sarc(p, n)) hos_log("zs: no dictionaries (%s)", path);
    else hos_log("zs: %d dictionaries", ndicts);
    free(p);
}

/* the dictionaries read the first time one is asked for. Every caller waits for that first read to end (a second one
   found none while the first was still reading) */
static void dicts_read(void) {
    static SRWLOCK lock = SRWLOCK_INIT;
    static int read;   /* (1 once read_dicts ended: published with release, seen with acquire) */
    if (!__atomic_load_n(&read, __ATOMIC_ACQUIRE)) {
        AcquireSRWLockExclusive(&lock);
        if (!read) {
            read_dicts();
            __atomic_store_n(&read, 1, __ATOMIC_RELEASE);
        }
        ReleaseSRWLockExclusive(&lock);
    }
}

/* The game's dictionary of this id, NULL none */
static const ZSTD_DDict *hos_zs_dict(unsigned id) {
    dicts_read();
    for (int i = 0; id && i < ndicts; i++)
        if (dicts[i].id == id) return dicts[i].dict;
    return NULL;
}

/* The game's dictionary of this name (a file of Pack/ZsDic.pack.zs: "zs.zsdic"), NULL none */
static const ZSTD_DDict *hos_zs_dict_named(const char *name) {
    dicts_read();
    for (int i = 0; name && i < ndicts; i++)
        if (!strcmp(dicts[i].name, name)) return dicts[i].dict;
    return NULL;
}

uint8_t *hos_zs_decode(const uint8_t *z, size_t zn, size_t *n, size_t limit) {
    const unsigned id = ZSTD_getDictID_fromFrame(z, zn);
    if (!id) return decode(z, zn, n, limit, NULL);
    const ZSTD_DDict *dict = hos_zs_dict(id);
    return dict ? decode(z, zn, n, limit, dict) : NULL;
}

/* hos_zs_decode, and for a frame that names no dictionary (id 0: zstd may leave the id out of a frame made with one)
   and is no plain frame either, the dictionary `named` (a file of Pack/ZsDic.pack.zs), as the host's readers named
   theirs before frames were matched by id: the sound archives zs.zsdic, the maps bcett.byml.zsdic */
uint8_t *hos_zs_decode_named(const uint8_t *z, size_t zn, size_t *n, size_t limit, const char *named) {
    uint8_t *d = hos_zs_decode(z, zn, n, limit);
    if (d || !named || ZSTD_getDictID_fromFrame(z, zn)) return d;
    const ZSTD_DDict *dict = hos_zs_dict_named(named);
    return dict ? decode(z, zn, n, limit, dict) : NULL;
}

uint8_t *hos_zs_read(const char *path, size_t *n, size_t limit) {
    size_t zn = 0;
    uint8_t *z = read_file(path, &zn), *d = z ? hos_zs_decode(z, zn, n, limit) : NULL;
    free(z);
    return d;
}

/* (longshot) romfs.c: a zstd-stored file of the .nup decoded at its open (a plain frame, the .nup's own wrapping) */
size_t hos_zs_frame(void *dst, size_t cap, const void *src, size_t n) {
    const size_t got = ZSTD_decompress(dst, cap, src, n);
    return ZSTD_isError(got) ? (size_t)-1 : got;
}

/* A decoder for streaming: a frame's window may be up to 2 GB (zstd's streaming decoder refuses one over 128 MB by
   default; the .nup's largest zstd-stored file is 117 MB, and a larger one would be refused) */
static ZSTD_DCtx *stream_ctx(void) {
    ZSTD_DCtx *ctx = ZSTD_createDCtx();
    if (ctx && ZSTD_isError(ZSTD_DCtx_setParameter(ctx, ZSTD_d_windowLogMax, 31))) {
        ZSTD_freeDCtx(ctx);
        return NULL;
    }
    return ctx;
}

/* (longshot) romfs.c: the first cap bytes a zstd-stored file of the .nup holds, from its frame's start (n bytes of it):
   only the blocks they are in are decoded */
size_t hos_zs_frame_head(void *dst, size_t cap, const void *src, size_t n) {
    ZSTD_DCtx *ctx = stream_ctx();
    if (!ctx) return (size_t)-1;
    ZSTD_inBuffer ib = {src, n, 0};
    ZSTD_outBuffer ob = {dst, cap, 0};
    size_t r = 1;
    while (ob.pos < cap && r && ib.pos < ib.size && !ZSTD_isError(r = ZSTD_decompressStream(ctx, &ob, &ib))) {}
    ZSTD_freeDCtx(ctx);
    return ZSTD_isError(r) || (ob.pos < cap && r) ? (size_t)-1 : ob.pos;
}

/* (longshot) the same for a file of the game's RomFS (romfs.h): rel its path there, with hos_zs_decode_named's fallback
   to the dictionary `named` (NULL: none) */
uint8_t *hos_zs_read_romfs_named(const char *rel, size_t *n, size_t limit, const char *named) {
    size_t zn = 0;
    uint8_t *z = hos_romfs_read(rel, &zn, 1u << 30), *d = z ? hos_zs_decode_named(z, zn, n, limit, named) : NULL;
    free(z);
    return d;
}
uint8_t *hos_zs_read_romfs(const char *rel, size_t *n, size_t limit) { return hos_zs_read_romfs_named(rel, n, limit, NULL); }

/* (the decoder of a .zs file as its bytes come: its dictionary by the id in its first bytes) */
typedef struct {
    ZSTD_DCtx *ctx;
    uint8_t *out;
    size_t out_cap, last;   /* (last: 0 once its frame ended) */
    int ok;
    int (*fn)(void *arg, const uint8_t *piece, size_t n);
    void *arg;
    uint8_t head[ZSTD_FRAMEHEADERSIZE_MAX];   /* (the frame's first bytes, until its header is whole) */
    size_t have;
} ZsStream;

static void zs_feed(ZsStream *z, const uint8_t *p, size_t n) {
    if (z->ok && !z->ctx) {   /* (the first bytes, kept until the frame's header is whole: it says the dictionary) */
        if (!n) {   /* (the end, the header never whole) */
            z->ok = 0;
            return;
        }
        const size_t k = n < sizeof z->head - z->have ? n : sizeof z->head - z->have;
        memcpy(z->head + z->have, p, k);
        z->have += k, p += k, n -= k;
        ZSTD_frameHeader fh;
        const size_t need = ZSTD_getFrameHeader(&fh, z->head, z->have);
        if (ZSTD_isError(need)) z->ok = 0;
        if (ZSTD_isError(need) || need) return;   /* (need: more bytes to come first) */
        const ZSTD_DDict *dict = fh.dictID ? hos_zs_dict(fh.dictID) : NULL;
        z->ctx = !fh.dictID || dict ? stream_ctx() : NULL;
        z->ok = z->ctx && (!dict || !ZSTD_isError(ZSTD_DCtx_refDDict(z->ctx, dict)));
        zs_feed(z, z->head, z->have);   /* (the header's bytes first, then the rest of this piece) */
        if (!n) return;
    }
    ZSTD_inBuffer ib = {p, n, 0};
    while (z->ok && (ib.pos < ib.size || (!n && z->last))) {
        ZSTD_outBuffer ob = {z->out, z->out_cap, 0};
        z->last = ZSTD_decompressStream(z->ctx, &ob, &ib);
        z->ok = !ZSTD_isError(z->last) && (!ob.pos || z->fn(z->arg, z->out, ob.pos));
        if (!n && !ob.pos) break;   /* (the end: nothing more came out) */
    }
}

/* (longshot) a .zs file of the game's RomFS unpacked as it streams: fn(arg, piece, n) for each piece in order, the file
   read a MB at a time (host/prompts.c: the textures' 244 MB, never whole in memory), and one the .nup keeps in a zstd
   frame of its own unwrapped as it streams too. 1 when the whole frame came and fn took every piece; 0 when there is
   no such file, it is not a frame this reads, or fn stopped */
int hos_zs_romfs_stream(const char *rel, int (*fn)(void *arg, const uint8_t *piece, size_t n), void *arg) {
    int framed = 0;
    HosRomfsFile *f = hos_romfs_open_stored(rel, &framed);
    if (!f) return 0;
    const uint64_t size = hos_romfs_file_size(f);
    const size_t in_cap = 1u << 20;
    ZsStream z = {NULL, (uint8_t *)malloc(ZSTD_DStreamOutSize()), ZSTD_DStreamOutSize(), 1, 1, fn, arg, {0}, 0};
    /* (the .nup's own frame round it, when it has one: unwrapped first) */
    ZSTD_DCtx *outer = framed ? stream_ctx() : NULL;
    uint8_t *in = (uint8_t *)malloc(in_cap), *mid = framed ? (uint8_t *)malloc(ZSTD_DStreamOutSize()) : NULL;
    z.ok = z.out && in && (!framed || (outer && mid));
    size_t outer_last = framed;
    for (uint64_t at = 0; z.ok && at < size;) {
        const int64_t n = hos_romfs_file_read(f, at, in, in_cap);
        if (n <= 0) {
            z.ok = 0;
            break;
        }
        at += (uint64_t)n;
        if (!framed) {
            zs_feed(&z, in, (size_t)n);
            continue;
        }
        ZSTD_inBuffer ib = {in, (size_t)n, 0};
        while (z.ok && ib.pos < ib.size) {
            ZSTD_outBuffer ob = {mid, ZSTD_DStreamOutSize(), 0};
            outer_last = ZSTD_decompressStream(outer, &ob, &ib);
            if (ZSTD_isError(outer_last)) z.ok = 0;
            else if (ob.pos) zs_feed(&z, mid, ob.pos);
        }
    }
    while (z.ok && outer_last) {   /* (what the .nup's frame still holds once its input is all in) */
        ZSTD_inBuffer ib = {in, 0, 0};
        ZSTD_outBuffer ob = {mid, ZSTD_DStreamOutSize(), 0};
        outer_last = ZSTD_decompressStream(outer, &ob, &ib);
        if (ZSTD_isError(outer_last) || !ob.pos) z.ok = 0;
        else zs_feed(&z, mid, ob.pos);
    }
    if (z.ok && z.last) zs_feed(&z, in, 0);   /* (what the file's frame still holds) */
    const int ok = z.ok && !z.last;
    if (z.ctx) ZSTD_freeDCtx(z.ctx);
    if (outer) ZSTD_freeDCtx(outer);
    free(in), free(mid), free(z.out);
    hos_romfs_file_close(f);
    return ok;
}

static uint64_t content_size(const uint8_t *head, size_t got, uint64_t size) {
    const unsigned long long c = ZSTD_getFrameContentSize(head, got);
    return c == ZSTD_CONTENTSIZE_ERROR ? size : c == ZSTD_CONTENTSIZE_UNKNOWN ? size * 4 : c;
}

/* (longshot) the size a .zs file of the game's RomFS decompresses to, as hos_zs_content_size says it; 0 when there is none */
uint64_t hos_zs_romfs_content_size(const char *rel) {
    uint8_t head[18];
    uint64_t size = 0;
    const int64_t got = hos_romfs_head(rel, head, sizeof head, &size);
    return got < 0 ? 0 : content_size(head, (size_t)got, size);
}

/* the size a .zs file decompresses to, as its frame says (the game's tools and zstd's write it), from its first bytes;
   4 times the bytes when the frame does not say; the bytes themselves for a file that is not a frame */
uint64_t hos_zs_content_size(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) return 0;
    uint8_t head[18];
    const size_t got = fread(head, 1, sizeof head, f);
    fseek(f, 0, SEEK_END);
    const long long size = _ftelli64(f);
    fclose(f);
    const unsigned long long c = ZSTD_getFrameContentSize(head, got);
    return c == ZSTD_CONTENTSIZE_ERROR ? (uint64_t)size : c == ZSTD_CONTENTSIZE_UNKNOWN ? (uint64_t)size * 4 : c;
}

/* data as a zstd frame of raw blocks of 128 KB (prompts_base.h's writer), written beside path and moved over it */
int hos_zs_write(const char *path, const uint8_t *data, size_t n) {
    char tmp[MAX_PATH];
    snprintf(tmp, sizeof tmp, "%s.new", path);
    FILE *f = fopen(tmp, "wb");
    if (!f) return 0;
    int ok = hos_zs_raw_head(f, n);
    size_t at = 0;
    do {
        const size_t k = n - at < 131072 ? n - at : 131072;
        ok = ok && hos_zs_raw_block(f, data + at, k, at + k == n);
        at += k;
    } while (at < n);
    ok &= fclose(f) == 0;
    return ok && MoveFileExA(tmp, path, MOVEFILE_REPLACE_EXISTING);
}
