/* The synthetic game's host: what the builder's end-to-end test (builder/crates/synthetic-game/tests/end_to_end.rs) links
   the lifted synthetic program with, in place of the game's host (host/, which boots the game itself: its OS, its
   renderer, its sites). It is built by the builder's compile as host/ is (every host/*.c of the tree, against the
   lift's gen/rt/a64.h), beside the real runtime (gen/rt/a64_rt.c, a64_fp.c) and the real host/romfs.c and
   host/romfs_index.c, which the test copies in.

   It maps guest memory where the lift's a64_config.h says, loads image.bin (the game folder's) at the image's base,
   opens the game folder's RomFS index (longshot_romfs.idx: the RomFS read in place from the .nup), calls nnMain with a
   stack, and prints what came back. The program calls it two ways: an SVC (a64_svc) and an import
   (testhost_romfs_read, bound by the image to imp_testhost_romfs_read, as the game's nn:: imports are to host/'s). */
#include "a64.h"
#include "romfs.h"
#include "romfs_index.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

/* the boot constants the lift writes (gen/image.c) */
extern const uint64_t hos_image_base, hos_image_end, hos_entry_nnMain, hos_main_stack_size;
extern const char hos_main_build_id[];

/* where host/romfs.c finds the RomFS (host/nn_fs.c's, in the game) */
const char *hos_romfs_dir;
HosRomfsIndex *hos_romfs_index;

/* what the lifted code asks of the host besides the runtime: the GPU's pages (none) and the interrupt poll (no
   interrupts) */
#if A64_MARK_BITMAP
uint8_t a64_gpu_pages[A64_DIRTY_PAGES / 8];
#endif
void a64_poll_cur(void) {}

/* the guest's stack: below the image, apart from the trampolines' addresses */
#define STACK_BASE 0x40000000ULL
#define STACK_SIZE 0x100000ULL

static int svc_calls, romfs_reads;

static void stop(const char *what, unsigned long long detail) {
    fprintf(stderr, "synthetic: %s (%#llx)\n", what, detail);
    fflush(stdout);
    _exit(3);
}

/* SVC #0x7f: x0 * 1000 + 1 back; any other is a mistake */
void a64_svc(Ctx *c, uint32_t imm) {
    if (imm != 0x7f) stop("an SVC the synthetic host does not answer", imm);
    svc_calls++;
    printf("synthetic: svc %#x with %llu\n", imm, (unsigned long long)c->x[0]);
    c->x[0] = c->x[0] * 1000 + 1;
}

void a64_on_trap(Ctx *c, uint64_t pc, uint32_t insn, const char *why) {
    (void)c;
    fprintf(stderr, "synthetic: trap at %#llx (%08x): %s\n", (unsigned long long)pc, insn, why ? why : "");
    _exit(4);
}

void a64_unimplemented(Ctx *c, const char *sym, int *seen) {
    (void)c;
    (void)seen;
    fprintf(stderr, "synthetic: a call to %s, which the host does not define\n", sym);
    _exit(5);
}

/* testhost_romfs_read(path, buf, cap): the RomFS file at path (guest memory) read into buf, at most cap bytes; its
   size, or -1 when there is no such file. Through host/romfs.c's API, as the game's file calls read it. */
void imp_testhost_romfs_read(Ctx *c) {
    const char *path = (const char *)A64_PTR(c->x[0]);
    uint8_t *buf = A64_PTR(c->x[1]);
    const uint64_t cap = c->x[2];
    romfs_reads++;
    HosRomfsFile *f = hos_romfs_open(path);
    if (!f) {
        printf("synthetic: romfs %s: none\n", path);
        c->x[0] = (uint64_t)-1;
        return;
    }
    const uint64_t size = hos_romfs_file_size(f);
    const int64_t got = hos_romfs_file_read(f, 0, buf, size < cap ? size : cap);
    hos_romfs_file_close(f);
    printf("synthetic: romfs %s: %lld of %llu bytes\n", path, (long long)got, (unsigned long long)size);
    c->x[0] = (uint64_t)got;
}

static void map(uint64_t guest, uint64_t size) {
    void *want = A64_PTR(guest);
    void *got = mmap(want, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
    if (got != want) stop("guest memory could not be mapped", guest);
}

int main(int argc, char **argv) {
    (void)argc;
    /* the game folder: the executable's own */
    char dir[4096];
    const ssize_t n = readlink("/proc/self/exe", dir, sizeof dir - 64);
    if (n <= 0) stop("no /proc/self/exe", 0);
    dir[n] = 0;
    char *slash = strrchr(dir, '/');
    if (slash) *slash = 0;
    (void)argv;
    printf("synthetic: build %s, image %#llx..%#llx\n", hos_main_build_id, (unsigned long long)hos_image_base,
           (unsigned long long)hos_image_end);

    char path[4200], why[512];
    snprintf(path, sizeof path, "%s/longshot_romfs.idx", dir);
    hos_romfs_index = hos_romfs_index_open(path, why, sizeof why);
    if (!hos_romfs_index) {
        fprintf(stderr, "synthetic: %s\n", why);
        return 2;
    }
    printf("synthetic: romfs index of %u files\n", hos_romfs_index_count(hos_romfs_index));

    const uint64_t span = (hos_image_end - hos_image_base + 0xFFF) & ~0xFFFULL;
    map(hos_image_base, span);
    map(STACK_BASE, STACK_SIZE);
    snprintf(path, sizeof path, "%s/image.bin", dir);
    FILE *f = fopen(path, "rb");
    if (!f) stop("no image.bin beside the game", 0);
    const size_t read = fread(A64_PTR(hos_image_base), 1, span, f);
    fclose(f);
    printf("synthetic: image.bin, %zu bytes\n", read);

    A64Fn entry = a64_lookup(hos_entry_nnMain);
    if (!entry) stop("no lifted function at nnMain", hos_entry_nnMain);
    static Ctx c;
    c.x[31] = STACK_BASE + STACK_SIZE - 16;
    entry(&c);
    printf("synthetic: %d svc calls, %d romfs reads\n", svc_calls, romfs_reads);
    printf("synthetic: result %#018llx\n", (unsigned long long)c.x[0]);
    fflush(stdout);
    return 0;
}
