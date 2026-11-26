/* host/prompts_base.c, what host/prompts.c serves the prompts' game files with: a .base the builder made
   (builder/crates/build-assets, prompts::base) read back, its bytes found where any run is, and the game's file patched as it
   streams in pieces of an odd size, every run of a textures.bin written over it (a later one over an earlier), out as a
   zstd frame of raw blocks. Built and run by builder/crates/build-assets (prompts::base, the_host_patches_the_stream), which
   makes the synthetic file, the runs and the base, decodes the frame and checks it, and checks a file that is not the
   base's (another game version) is refused.

       prompts_base_test <base> <the game's file> <textures.bin> <out>     exit 0: written; 1: refused (not the base's file); 2: the base wrong; 3: the base refused */
#include "../host/prompts_base.h"
#include <stdlib.h>
#include <string.h>

static unsigned char *slurp(const char *path, size_t *n) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    *n = (size_t)ftell(f);
    fseek(f, 0, SEEK_SET);
    unsigned char *b = (unsigned char *)malloc(*n + 1);
    if (b && fread(b, 1, *n, f) != *n) free(b), b = NULL;
    fclose(f);
    return b;
}

static uint32_t rd32(const unsigned char *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

static int run_cmp(const void *a, const void *b) {
    const HosPatchRun *x = (const HosPatchRun *)a, *y = (const HosPatchRun *)b;
    return x->off != y->off ? (x->off < y->off ? -1 : 1) : (x->idx > y->idx) - (x->idx < y->idx);
}

int main(int argc, char **argv) {
    if (argc != 5) return 2;
    size_t bn = 0, gn = 0, tn = 0;
    unsigned char *bytes = slurp(argv[1], &bn), *game = slurp(argv[2], &gn), *t = slurp(argv[3], &tn);
    HosPromptsBase base;
    if (!game || !t) return 2;
    if (!hos_prompts_base_parse(&base, bytes, bn)) return 3;
    /* every run's range is in the base (the patch checks its bytes against the file's as it streams) */
    HosPatchRun *runs = (HosPatchRun *)malloc(4096 * sizeof *runs);
    size_t n = 0, at = 8;
    for (uint32_t e = 0, count = rd32(t + 4); e < count && at + 8 <= tn; e++) {
        const uint32_t nr = rd32(t + at + 4);
        at += 8;
        for (uint32_t r = 0; r < nr && n < 4096; r++) {
            runs[n] = (HosPatchRun){rd32(t + at), rd32(t + at + 4), (uint32_t)n, t + at + 8};
            unsigned char *got = (unsigned char *)malloc(runs[n].len + 1);
            if (!hos_prompts_base_at(&base, runs[n].off, got, runs[n].len)) {
                fprintf(stderr, "FAIL run %zu not in the base\n", n);
                return 2;
            }
            free(got);
            at += 8 + runs[n].len;
            n++;
        }
    }
    unsigned char one;
    if (hos_prompts_base_at(&base, base.size, &one, 1)) return 2;   /* (past the file: nothing) */
    unsigned char two[2];
    if (hos_prompts_base_at(&base, UINT64_MAX, two, 2) || hos_prompts_base_at(&base, 0, two, UINT64_MAX)) return 2;   /* (an end past 2^64: nothing) */
    qsort(runs, n, sizeof *runs, run_cmp);
    FILE *out = fopen(argv[4], "wb");
    HosPatch *p = out ? hos_patch_begin(out, &base, runs, n) : NULL;
    int ok = p != NULL;
    for (size_t k = 0; ok && k < gn; k += 7777) ok = hos_patch_feed(p, game + k, gn - k < 7777 ? gn - k : 7777);
    if (p) ok &= hos_patch_end(p);
    if (out) fclose(out);
    hos_prompts_base_free(&base);
    free(runs), free(game), free(t);
    return ok ? 0 : 1;
}
