/* The runtime the new lifter's single-instruction functions link against in tests/test_lifter_semantics.py: the
   soft write watch's table, stand-ins for the host's calls (none of which the checked instructions reach), and a
   window page mapped where lifted code finds guest memory (A64_HOST_BASE + guest address). */
#define _GNU_SOURCE
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>

#define HOST_BASE 0x100000000000ULL

uint8_t a64_dirty[1u << 27];   /* (llvm::Options::default's dirty_pages: untouched pages cost nothing) */
int trapped;
uint64_t lifter_fast_checks, lifter_slow_checks;   /* (longshot-builder dev words --count-checks: the float fast paths' runs) */

void a64_trap(void *c, uint64_t pc, uint32_t word) { (void)c; (void)pc; (void)word; trapped = 1; }
void a64_svc(void *c, uint32_t imm) { (void)c; (void)imm; trapped = 2; }
void a64_poll_cur(void) {}
void *a64_lookup(uint64_t addr) { (void)addr; return 0; }
void a64_icall_site(void *c, uint64_t t, uint64_t *site) { (void)c; (void)t; (void)site; trapped = 3; }
void a64_ijump_site(void *c, uint64_t t, uint64_t *site) { (void)c; (void)t; (void)site; trapped = 4; }
uint64_t recomp_ldxr(void *c, uint64_t a, uint32_t n) { (void)c; (void)a; (void)n; trapped = 5; return 0; }
uint32_t recomp_stxr(void *c, uint64_t a, uint32_t n, uint64_t v) { (void)c; (void)a; (void)n; (void)v; trapped = 5; return 1; }
void recomp_ldxp(void *c, uint64_t a, uint32_t n, uint64_t *lo, uint64_t *hi) { (void)c; (void)a; (void)n; *lo = *hi = 0; trapped = 5; }
uint32_t recomp_stxp(void *c, uint64_t a, uint32_t n, uint64_t lo, uint64_t hi) { (void)c; (void)a; (void)n; (void)lo; (void)hi; trapped = 5; return 1; }
void recomp_clrex(void *c) { (void)c; }
uint64_t recomp_cntpct(void *c) { (void)c; return 0; }

/* The guest range [guest, guest + size) mapped read-write in the window: 0, or -1 if the host would not. */
int map_window(uint64_t guest, uint64_t size) {
    void *want = (void *)(uintptr_t)(HOST_BASE + guest);
    void *got = mmap(want, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
    return got == want ? 0 : -1;
}

void window_write(uint64_t guest, const void *src, uint64_t size) { memcpy((void *)(uintptr_t)(HOST_BASE + guest), src, size); }
void window_read(uint64_t guest, void *dst, uint64_t size) { memcpy(dst, (const void *)(uintptr_t)(HOST_BASE + guest), size); }
