/* (longshot) prompts_base.h: the prompts' game files read as they stream from the RomFS, and what the builder keeps of
   them (the .base files). Plain C over stdio, with nothing of the host's: tests/prompts_base_test.c builds it alone. */
#include "prompts_base.h"
#include <stdlib.h>
#include <string.h>

static uint32_t rd32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }
static uint64_t rd64(const uint8_t *p) { return rd32(p) | (uint64_t)rd32(p + 4) << 32; }
static void wr32(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v, p[1] = (uint8_t)(v >> 8), p[2] = (uint8_t)(v >> 16), p[3] = (uint8_t)(v >> 24); }

int hos_prompts_base_parse(HosPromptsBase *b, uint8_t *bytes, size_t n) {
    memset(b, 0, sizeof *b);
    if (!bytes || n < 24 || memcmp(bytes, "HPBS", 4) || rd32(bytes + 4) != 1) {
        free(bytes);
        return 0;
    }
    const uint32_t ns = rd32(bytes + 16);
    if (ns > (n - 20) / 8 || 20 + 8 * (size_t)ns + 4 > n) {
        free(bytes);
        return 0;
    }
    size_t at = 20 + 8 * (size_t)ns;
    const uint32_t nr = rd32(bytes + at);
    at += 4;
    const size_t runs_at = at;
    uint64_t last = 0;
    for (uint32_t r = 0; r < nr; r++) {   /* (whole, in order, apart, never touching: base_at finds a range in one run,
                                              and the builder merges runs that touch) */
        const uint32_t off = at + 8 <= n ? rd32(bytes + at) : 0, len = at + 8 <= n ? rd32(bytes + at + 4) : 0;
        if (at + 8 > n || len > n - at - 8 || (r && off <= last) || (uint64_t)off + len > rd64(bytes + 8)) {
            free(bytes);
            return 0;
        }
        last = (uint64_t)off + len;
        at += 8 + (size_t)len;
    }
    if (at != n) {
        free(bytes);
        return 0;
    }
    *b = (HosPromptsBase){bytes, n, rd64(bytes + 8), ns, bytes + 20, nr, bytes + runs_at};
    return 1;
}

void hos_prompts_base_free(HosPromptsBase *b) {
    free(b->bytes);
    memset(b, 0, sizeof *b);
}

uint64_t hos_prompts_base_sample(const HosPromptsBase *b, uint32_t k) {
    return k < b->nsamples ? rd64(b->samples + 8 * (size_t)k) : 0;
}

int hos_prompts_base_at(const HosPromptsBase *b, uint64_t off, void *dst, uint64_t n) {
    const uint8_t *p = b->runs;
    for (uint32_t r = 0; r < b->nruns; r++) {
        const uint64_t o = rd32(p), len = rd32(p + 4);
        if (off >= o && n <= len && off - o <= len - n) {   /* (off + n would wrap for an off near 2^64) */
            memcpy(dst, p + 8 + (off - o), (size_t)n);
            return 1;
        }
        if (o > off) return 0;
        p += 8 + len;
    }
    return 0;
}

enum { BLOCK = 131072 };

int hos_zs_raw_head(FILE *out, uint64_t n) {
    if (n >= 0xFFFFFFFFull) return 0;
    uint8_t head[10];
    wr32(head, 0xFD2FB528u);
    head[4] = 0x80;     /* (FCS 4 bytes, not single segment, no checksum, no dictionary) */
    head[5] = 7 << 3;   /* (window 2^(10+7)) */
    wr32(head + 6, (uint32_t)n);
    return fwrite(head, 1, 10, out) == 10;
}

int hos_zs_raw_block(FILE *out, const uint8_t *data, size_t k, int last) {
    const uint32_t bh = (uint32_t)(k << 3) | (last ? 1u : 0u);
    const uint8_t b3[3] = {(uint8_t)bh, (uint8_t)(bh >> 8), (uint8_t)(bh >> 16)};
    return fwrite(b3, 1, 3, out) == 3 && fwrite(data, 1, k, out) == k;
}

struct HosPatch {
    FILE *out;
    const HosPromptsBase *base;
    const HosPatchRun *runs;
    size_t nruns, first;       /* (first: the first run that may reach the chunk) */
    const uint8_t *next_base;  /* (the next base run to check, and how many are left) */
    uint32_t base_left;
    const HosPatchRun **here;
    uint8_t *chunk;
    size_t have;               /* bytes in chunk */
    uint64_t pos;              /* the file's offset of chunk[0] */
    int ok;
};

HosPatch *hos_patch_begin(FILE *out, const HosPromptsBase *base, const HosPatchRun *runs, size_t nruns) {
    HosPatch *p = (HosPatch *)calloc(1, sizeof *p);
    if (!p) return NULL;
    *p = (HosPatch){out, base, runs, nruns, 0, base->runs, base->nruns, NULL, NULL, 0, 0, 1};
    p->here = (const HosPatchRun **)malloc((nruns ? nruns : 1) * sizeof *p->here);
    p->chunk = (uint8_t *)malloc(BLOCK);
    if (!p->here || !p->chunk || !hos_zs_raw_head(out, base->size)) {
        free(p->here), free(p->chunk), free(p);
        return NULL;
    }
    return p;
}

static int by_idx(const void *a, const void *b) {
    const uint32_t x = (*(const HosPatchRun *const *)a)->idx, y = (*(const HosPatchRun *const *)b)->idx;
    return (x > y) - (x < y);
}

/* the chunk (k bytes at pos) checked against the base, the runs written over it, out as one raw block */
static int flush(HosPatch *p, size_t k) {
    const uint64_t pos = p->pos, end = pos + k;
    while (p->base_left) {   /* (the base's bytes that fall here: the stream's must be the same) */
        const uint64_t o = rd32(p->next_base), len = rd32(p->next_base + 4);
        if (o >= end) break;
        const uint64_t a = o > pos ? o : pos, z = o + len < end ? o + len : end;
        if (z > a && memcmp(p->chunk + (a - pos), p->next_base + 8 + (a - o), (size_t)(z - a))) return 0;
        if (o + len > end) break;   /* (the rest of it in the next chunk) */
        p->next_base += 8 + len;
        p->base_left--;
    }
    while (p->first < p->nruns && (uint64_t)p->runs[p->first].off + p->runs[p->first].len <= pos) p->first++;
    size_t nh = 0;   /* (the runs in this chunk, written in their order: a later one over an earlier) */
    for (size_t r = p->first; r < p->nruns && p->runs[r].off < end; r++)
        if ((uint64_t)p->runs[r].off + p->runs[r].len > pos) p->here[nh++] = &p->runs[r];
    qsort(p->here, nh, sizeof *p->here, by_idx);
    for (size_t i = 0; i < nh; i++) {
        const HosPatchRun *r = p->here[i];
        const uint64_t a = r->off > pos ? r->off : pos;
        const uint64_t z = (uint64_t)r->off + r->len < end ? (uint64_t)r->off + r->len : end;
        memcpy(p->chunk + (a - pos), r->data + (a - r->off), (size_t)(z - a));
    }
    if (!hos_zs_raw_block(p->out, p->chunk, k, end == p->base->size)) return 0;
    p->pos = end;
    p->have = 0;
    return 1;
}

int hos_patch_feed(void *patch, const uint8_t *data, size_t n) {
    HosPatch *p = (HosPatch *)patch;
    while (p->ok && n) {
        if (p->pos + p->have + n > p->base->size) return p->ok = 0;   /* (longer than the base's file) */
        const size_t k = BLOCK - p->have < n ? BLOCK - p->have : n;
        memcpy(p->chunk + p->have, data, k);
        p->have += k, data += k, n -= k;
        if (p->have == BLOCK || p->pos + p->have == p->base->size) p->ok = flush(p, p->have);
    }
    return p->ok;
}

int hos_patch_end(HosPatch *p) {
    const int ok = p->ok && !p->have && p->pos == p->base->size && p->base->size > 0;
    free(p->here), free(p->chunk), free(p);
    return ok;
}
