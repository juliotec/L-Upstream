/* Exact single-precision arithmetic for host code that stands in for guest instructions (game/oot_sites.c), under the
   lifter's float model (builder/crates/sem/src/fp.rs): rounding to nearest, FPCR.FZ (subnormal operands and results are
   zeros of their sign) and FPCR.DN (every NaN result the default NaN) read from the context, Arm's NaNs (a signalling
   operand first, in operand order, quieted; else the first quiet one; an invalid operation gives 0x7fc00000), and no
   cumulative FPSR flags. Written into gen/rt/ by `longshot-builder dev lift`. */
#include "a64.h"
#include <math.h>

#define FZ_BIT (1u << 24)
#define DN_BIT (1u << 25)
#define DEFAULT_NAN 0x7fc00000u

static float f_of(uint32_t u) { float f; memcpy(&f, &u, 4); return f; }
static uint32_t u_of(float f) { uint32_t u; memcpy(&u, &f, 4); return u; }
static int is_nan(uint32_t u) { return (u & 0x7f800000u) == 0x7f800000u && (u & 0x007fffffu); }
static int is_snan(uint32_t u) { return is_nan(u) && !(u & 0x00400000u); }
static int is_inf(uint32_t u) { return (u & 0x7fffffffu) == 0x7f800000u; }
static int is_zero(uint32_t u) { return (u & 0x7fffffffu) == 0; }

static uint32_t flush(uint64_t fpcr, uint32_t u) {
    return (fpcr & FZ_BIT) && (u & 0x7f800000u) == 0 ? u & 0x80000000u : u;
}

/* FPProcessNaNs over ops (n of them, one at least a NaN) */
static uint32_t nan_of(uint64_t fpcr, const uint32_t *ops, int n) {
    if (fpcr & DN_BIT) return DEFAULT_NAN;
    for (int i = 0; i < n; i++)
        if (is_snan(ops[i])) return ops[i] | 0x00400000u;
    for (int i = 0; i < n; i++)
        if (is_nan(ops[i])) return ops[i] | 0x00400000u;
    return DEFAULT_NAN;
}

/* the host's result r with Arm's NaN where it is one, then flushed */
static uint64_t finish(uint64_t fpcr, uint32_t r, const uint32_t *ops, int n) {
    if (is_nan(r)) {
        int any = 0;
        for (int i = 0; i < n; i++) any |= is_nan(ops[i]);
        r = any ? nan_of(fpcr, ops, n) : DEFAULT_NAN;
    }
    return flush(fpcr, r);
}

uint64_t recomp_fpe_fma32(Ctx *c, uint64_t a, uint64_t b, uint64_t addend) {
    const uint64_t fpcr = c->fpcr;
    const uint32_t ops[3] = {flush(fpcr, (uint32_t)addend), flush(fpcr, (uint32_t)a), flush(fpcr, (uint32_t)b)};
    const int inf_zero = (is_inf(ops[1]) && is_zero(ops[2])) || (is_zero(ops[1]) && is_inf(ops[2]));
    if (inf_zero && is_nan(ops[0]) && !is_snan(ops[0])) return DEFAULT_NAN;   /* (FPMulAdd: a quiet NaN addend too) */
    return finish(fpcr, u_of(fmaf(f_of(ops[1]), f_of(ops[2]), f_of(ops[0]))), ops, 3);
}

uint64_t recomp_fpe_div32(Ctx *c, uint64_t a, uint64_t b) {
    const uint64_t fpcr = c->fpcr;
    const uint32_t ops[2] = {flush(fpcr, (uint32_t)a), flush(fpcr, (uint32_t)b)};
    return finish(fpcr, u_of(f_of(ops[0]) / f_of(ops[1])), ops, 2);
}
