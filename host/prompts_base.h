/* (longshot) host/prompts.c's two game files without a copy of them in the game folder: the textures
   (UI/Layout2Archive/Common.*.bntx.zs) and the layouts (Common.*.blarc.zs) are read from the game's RomFS (romfs.h) as
   they stream out of their zstd frames, and the builder keeps only what prompts.c needs of them besides:

     prompts/<Common.bntx | Common.blarc>.base: "HPBS", u32 version (1), u64 the game's file's size unpacked,
         u32 samples, u64 each sample's offset (bytes no run of any face writes: the textures' pool is known by them),
         u32 runs, then each: u32 offset, u32 length, the game's bytes there

   The runs hold the game's bytes under every byte any face's textures.bin (or ring.bin and hint.bin) may write, merged,
   and the bytes prompts.c reads to know the file by (its header, the samples): what a change while the game runs puts
   back, and what a run written only over the game's bytes compares with. A file made from another game version is
   refused: its size differs, or (as it streams) its bytes under the runs do.

   hos_patch_* write the file as served (cache/prompts/<file>.zs): the game's bytes as they stream, the runs chosen
   written over them (a later run over an earlier where they meet), as a zstd frame of raw blocks of 128 KB. */
#ifndef HOS_PROMPTS_BASE_H
#define HOS_PROMPTS_BASE_H
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

typedef struct {
    uint8_t *bytes;         /* the file, owned */
    size_t n;
    uint64_t size;          /* the game's file's size, unpacked */
    uint32_t nsamples;
    const uint8_t *samples; /* nsamples u64 offsets */
    uint32_t nruns;
    const uint8_t *runs;    /* the runs, in the file's order (by offset, none touching another: a base with runs that
                               touch is refused, as the builder never writes one) */
} HosPromptsBase;

/* bytes (malloc'd, taken: freed with the base) read as a base; 0 when they are not one (bytes freed) */
int hos_prompts_base_parse(HosPromptsBase *b, uint8_t *bytes, size_t n);
void hos_prompts_base_free(HosPromptsBase *b);
/* the game's bytes [off, off + n) into dst: 1 when the base has them all (inside one run), else 0 */
int hos_prompts_base_at(const HosPromptsBase *b, uint64_t off, void *dst, uint64_t n);
/* sample k's offset */
uint64_t hos_prompts_base_sample(const HosPromptsBase *b, uint32_t k);

/* a run written over the game's file: (offset, length, its place in the list chosen: a later one over an earlier) */
typedef struct { uint32_t off, len, idx; const uint8_t *data; } HosPatchRun;

typedef struct HosPatch HosPatch;
/* the patch of the game's file into out (base: its size and the bytes checked as they come; runs sorted by offset, then
   by idx), NULL when it cannot be begun */
HosPatch *hos_patch_begin(FILE *out, const HosPromptsBase *base, const HosPatchRun *runs, size_t nruns);
/* the game's next n bytes: 0 when the stream is not the base's file (or a write failed) */
int hos_patch_feed(void *patch, const uint8_t *data, size_t n);
/* the patch ended (freed): 1 when the whole file came, as the base says it, and was written */
int hos_patch_end(HosPatch *p);

/* The one writer of a zstd frame of raw blocks (the patch above, zs.c's hos_zs_write): the header for n bytes (the
   4-byte content size, a window of 128 KB: raw blocks never look back, and a single-segment frame's window, the whole
   content, is more than decoders take: zstd's own refuses 244 MB); 0 when n does not fit in 4 bytes */
int hos_zs_raw_head(FILE *out, uint64_t n);
/* one raw block of it: k bytes (at most 128 KB), last for the frame's last */
int hos_zs_raw_block(FILE *out, const uint8_t *data, size_t k, int last);
#endif
