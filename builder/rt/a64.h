/* The guest model every lifted function and the host share: the CPU context, guest memory, the page marks, and the
   runtime's entry points (a64_rt.c). Written into gen/rt/ by `longshot-builder dev lift`
   (docs/dev/reference/builder.md, "The gen/ folder"), with a64_config.h beside it: the choices the lift made (polls,
   the GPU page bitmap, the window's base and size), so the host is compiled against the same model as the IR it links
   with.

   The lifted code is LLVM IR (builder/crates/llvm): it does not include this header, it has the same layout and calls the
   same symbols (builder/crates/llvm/src/lib.rs `ctx` and `RUNTIME`). The host's C (host/, gpu/) includes it.

   After a64-kit's rt/a64.h (NotRexed, TOTKPort) with Hookshot's and Longshot's changes: its context layout, kept
   field for field, and its 64-bit window (written into gen/rt/ by longshot-builder's lift). */
#ifndef A64_H
#define A64_H
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "a64_config.h"

/* ---- the context: the guest's registers, as lifted functions see them ------------------------------------------ */
typedef struct GuestContext {
    uint64_t x[32];             /* X0-X30; x[31] is SP */
    uint64_t pc;                /* written only where a thread stops (a trap, a call to no function) */
    uint8_t n, z, c, v;         /* NZCV, a byte each */
    int halted;
    uint64_t vreg[32][2];       /* Q0-Q31, two 64-bit halves each */
    uint64_t tpidr_el0;         /* the guest's own thread pointer (C++ TLS) */
    uint64_t tpidrro_el0;       /* Horizon's thread-local region, set by the OS layer */
    uint64_t fpcr, fpsr;
    /* the exclusive monitor (LDXR/STXR): the address, size and value LDXR saw; STXR compares and swaps against that
       value, so it holds across real host threads */
    uint64_t excl_addr, excl_lo, excl_hi;
    uint32_t excl_size;
    int excl_valid;
    void *thread;               /* the OS layer's thread object */
    uint32_t poll;              /* an interrupt request: a back edge clears it and calls a64_poll_cur when it is set */
    uint32_t piece;             /* which entry block the next piece of a split function starts at (passes::split) */
} GuestContext;
typedef GuestContext Ctx;

/* (the IR's offsets: builder/crates/llvm/src/lib.rs `ctx`) */
_Static_assert(offsetof(GuestContext, vreg) == 272 && offsetof(GuestContext, fpcr) == 800 &&
               offsetof(GuestContext, poll) == 856 && offsetof(GuestContext, piece) == 860 &&
               sizeof(((GuestContext *)0)->poll) == 4 && sizeof(((GuestContext *)0)->piece) == 4 &&
               sizeof(GuestContext) == 864, "the lifted code's context layout");

/* Every context parameter the lifted code may call through: none keeps the pointer past the call, as the IR's
   `nocapture` says (but recomp_sj_push's, whose context's address finds its slots: the IR says nothing of it). */
#ifndef A64_NOESC
#if defined(__clang__)
#define A64_NOESC __attribute__((noescape))
#else
#define A64_NOESC
#endif
#endif

#define A64_INLINE static inline __attribute__((always_inline))

/* ---- guest memory: one window of the 64-bit guest space at a constant host address ------------------------------ */
#ifdef A64_MEM_ZERO
#define A64_MEM ((uint8_t *)0)
#else
#define A64_MEM ((uint8_t *)A64_HOST_BASE)
#endif
#define A64_PTR(a) (A64_MEM + (uint64_t)(a))

/* The soft write watch: a store marks the 4 KB pages of its first and last byte (a release store, so a scan that
   clears a mark has seen the data it marks). With A64_MARK_BITMAP only a store whose first or last byte is on a page the
   GPU maps marks: host/mark_bitmap.c keeps that bitmap. The lifted code marks the same way (builder/crates/llvm `mark`). */
extern uint8_t a64_dirty[A64_DIRTY_PAGES];
#define A64_DIRTY_AT(a) (((uint64_t)(a) >> 12) & (A64_DIRTY_PAGES - 1))
#if defined(A64_NO_MARKS)
#define A64_MARK(a, n) ((void)0)
#elif A64_MARK_BITMAP
extern uint8_t a64_gpu_pages[A64_DIRTY_PAGES / 8];
#define A64_GPU_PAGE(a) ((a64_gpu_pages[A64_DIRTY_AT(a) >> 3] >> (A64_DIRTY_AT(a) & 7)) & 1)
#define A64_MARK(a, n) ((A64_GPU_PAGE(a) | A64_GPU_PAGE((a) + (n) - 1)) ? \
                        (void)(__atomic_store_n(&a64_dirty[A64_DIRTY_AT(a)], 1, __ATOMIC_RELEASE), \
                               __atomic_store_n(&a64_dirty[A64_DIRTY_AT((a) + (n) - 1)], 1, __ATOMIC_RELEASE)) : (void)0)
#else
#define A64_MARK(a, n) (__atomic_store_n(&a64_dirty[A64_DIRTY_AT(a)], 1, __ATOMIC_RELEASE), \
                        __atomic_store_n(&a64_dirty[A64_DIRTY_AT((a) + (n) - 1)], 1, __ATOMIC_RELEASE))
#endif

/* The host's reads and writes of guest memory (stores mark their pages; the _sp ones are the guest stack's, which the
   GPU never reads) */
A64_INLINE uint64_t recomp_load8(GuestContext *c, uint64_t a) { (void)c; return *(const uint8_t *)A64_PTR(a); }
A64_INLINE uint64_t recomp_load16(GuestContext *c, uint64_t a) { uint16_t v; (void)c; memcpy(&v, A64_PTR(a), 2); return v; }
A64_INLINE uint64_t recomp_load32(GuestContext *c, uint64_t a) { uint32_t v; (void)c; memcpy(&v, A64_PTR(a), 4); return v; }
A64_INLINE uint64_t recomp_load64(GuestContext *c, uint64_t a) { uint64_t v; (void)c; memcpy(&v, A64_PTR(a), 8); return v; }
A64_INLINE void recomp_store8_sp(GuestContext *c, uint64_t a, uint64_t v) { (void)c; *(uint8_t *)A64_PTR(a) = (uint8_t)v; }
A64_INLINE void recomp_store16_sp(GuestContext *c, uint64_t a, uint64_t v) { uint16_t t = (uint16_t)v; (void)c; memcpy(A64_PTR(a), &t, 2); }
A64_INLINE void recomp_store32_sp(GuestContext *c, uint64_t a, uint64_t v) { uint32_t t = (uint32_t)v; (void)c; memcpy(A64_PTR(a), &t, 4); }
A64_INLINE void recomp_store64_sp(GuestContext *c, uint64_t a, uint64_t v) { (void)c; memcpy(A64_PTR(a), &v, 8); }
A64_INLINE void recomp_store8(GuestContext *c, uint64_t a, uint64_t v) { recomp_store8_sp(c, a, v); A64_MARK(a, 1); }
A64_INLINE void recomp_store16(GuestContext *c, uint64_t a, uint64_t v) { recomp_store16_sp(c, a, v); A64_MARK(a, 2); }
A64_INLINE void recomp_store32(GuestContext *c, uint64_t a, uint64_t v) { recomp_store32_sp(c, a, v); A64_MARK(a, 4); }
A64_INLINE void recomp_store64(GuestContext *c, uint64_t a, uint64_t v) { recomp_store64_sp(c, a, v); A64_MARK(a, 8); }

/* A Q register's low 64 bits set and its high half cleared, as a scalar write does */
A64_INLINE void a64_vset64(uint64_t *d, uint64_t v) { d[0] = v; d[1] = 0; }

/* ---- control ------------------------------------------------------------------------------------------------- */
typedef void (*A64Fn)(Ctx *A64_NOESC);
#define TAILCALL(f) __attribute__((musttail)) return f(c)

A64Fn a64_lookup(uint64_t addr);                                  /* the function at a guest address, or NULL */
void a64_icall(Ctx *A64_NOESC c, uint64_t target);                /* a call through a register, uncached */
void a64_ijump(Ctx *A64_NOESC c, uint64_t target);                /* a jump through a register (an indirect tail call) */
/* BLR and BR in the lifted code: a cache of two targets per site (the guest address in the top 32 bits, the function
   as an offset from a64_lookup in the low 32), the site's first slot tried inline, these on a miss */
void a64_icall_site(Ctx *A64_NOESC c, uint64_t target, uint64_t *site);
void a64_ijump_site(Ctx *A64_NOESC c, uint64_t target, uint64_t *site);
void a64_svc(Ctx *A64_NOESC c, uint32_t imm);                     /* SVC #imm: the OS layer's (host/hos_svc.c) */
void a64_trap(Ctx *A64_NOESC c, uint64_t pc, uint32_t insn);      /* BRK, HLT, UDF, words that are no instruction */

/* What the OS layer may define (weak defaults in a64_rt.c) */
void a64_on_trap(Ctx *c, uint64_t pc, uint32_t insn, const char *why);   /* a thread cannot go on */
int a64_call_miss(Ctx *c, uint64_t target);                       /* a call to no lifted function: 1 if it ran it */
A64Fn a64_lookup_host(uint64_t addr);                             /* host functions at addresses the OS layer made */
void a64_ijump_hook(Ctx *c, uint64_t target);                     /* asked first by every uncached BR (longjmp) */
uint64_t a64_cntpct_host(void);                                   /* the 19.2 MHz counter */
void a64_unimplemented(Ctx *A64_NOESC c, const char *sym, int *seen);   /* an import stub's (gen/imports.c) */

/* setjmp in lifted code (host/hos_setjmp.c): a call of a function that calls __setjmp pushes a slot with its setjmp
   return addresses, takes a host setjmp on it, and runs the owner's code from the return address a guest longjmp
   came back to */
void **recomp_sj_push(Ctx *c, uint64_t r0, uint64_t r1, uint64_t r2, uint64_t r3);
uint64_t recomp_sj_target(void **slot);
void recomp_sj_pop(void **slot);

/* The exclusive monitor and the counter, as the IR calls them */
uint64_t recomp_ldxr(GuestContext *A64_NOESC c, uint64_t addr, uint32_t size);
uint32_t recomp_stxr(GuestContext *A64_NOESC c, uint64_t addr, uint32_t size, uint64_t value);
void recomp_ldxp(GuestContext *A64_NOESC c, uint64_t addr, uint32_t size, uint64_t *lo, uint64_t *hi);
uint32_t recomp_stxp(GuestContext *A64_NOESC c, uint64_t addr, uint32_t size, uint64_t lo, uint64_t hi);
void recomp_clrex(GuestContext *A64_NOESC c);
uint64_t recomp_cntpct(GuestContext *A64_NOESC c);

/* Exact single-precision arithmetic for host code that stands in for guest instructions (game/oot_sites.c): the
   lifter's float model (builder/crates/sem/src/fp.rs: FPCR.FZ and DN read, Arm's NaNs, no cumulative flags), in a64_fp.c.
   Bits in, bits out (the low 32 bits of each). recomp_fpe_fma32 is addend + a * b rounded once (FPMulAdd: NaNs in
   the order addend, a, b); a multiply is it with a zero addend of the product's sign. */
uint64_t recomp_fpe_fma32(Ctx *A64_NOESC c, uint64_t a, uint64_t b, uint64_t addend);
uint64_t recomp_fpe_div32(Ctx *A64_NOESC c, uint64_t a, uint64_t b);

/* ---- interrupt polls: the lifted back edges' (the IR's `poll`), and the host's own long loops ------------------ */
void a64_poll_cur(void);           /* the OS layer's: finds the thread's context itself (host/mkw_glue.c) */
#if A64_POLLING
#define A64_POLL(c) do { if (__builtin_expect(__atomic_load_n(&(c)->poll, __ATOMIC_RELAXED) != 0, 0)) { \
    __atomic_store_n(&(c)->poll, 0, __ATOMIC_RELAXED); a64_poll_cur(); } } while (0)
#else
#define A64_POLL(c) ((void)0)
#endif
#endif
