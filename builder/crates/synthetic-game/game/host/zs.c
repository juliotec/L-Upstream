/* The synthetic game's host: host/romfs.c's calls into host/zs.c and host/hos_thread.c (which need the game's host
   around them), the same as theirs: a zstd-stored file of the .nup decoded at its open, its first bytes from its
   frame's first blocks (a window up to 2 GB, as zs.c's stream_ctx), by third_party/zstd's decoder (the compile builds
   it into every game, and gives a file of this name its headers); hos_log to stderr. */
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#define ZSTD_STATIC_LINKING_ONLY
#include "zstd.h"

size_t hos_zs_frame(void *dst, size_t cap, const void *src, size_t n);
size_t hos_zs_frame_head(void *dst, size_t cap, const void *src, size_t n);
void hos_log(const char *fmt, ...);

size_t hos_zs_frame(void *dst, size_t cap, const void *src, size_t n) {
    const size_t got = ZSTD_decompress(dst, cap, src, n);
    return ZSTD_isError(got) ? (size_t)-1 : got;
}

size_t hos_zs_frame_head(void *dst, size_t cap, const void *src, size_t n) {
    ZSTD_DCtx *ctx = ZSTD_createDCtx();
    if (!ctx || ZSTD_isError(ZSTD_DCtx_setParameter(ctx, ZSTD_d_windowLogMax, 31))) {
        ZSTD_freeDCtx(ctx);
        return (size_t)-1;
    }
    ZSTD_inBuffer ib = {src, n, 0};
    ZSTD_outBuffer ob = {dst, cap, 0};
    size_t r = 1;
    while (ob.pos < cap && r && ib.pos < ib.size && !ZSTD_isError(r = ZSTD_decompressStream(ctx, &ob, &ib))) {}
    ZSTD_freeDCtx(ctx);
    return ZSTD_isError(r) || (ob.pos < cap && r) ? (size_t)-1 : ob.pos;
}

void hos_log(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    fputc('\n', stderr);
}
