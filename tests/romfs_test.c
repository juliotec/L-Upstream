/* host/romfs.c, the host's one API over the game's RomFS, on both its backends: the .nup's index (host/romfs_index.c,
   zstd-stored files decoded at open with third_party/zstd) and a romfs folder. Against a romfs unpacked from the same
   .nup: every file found, of its size, read whole and in pieces from four threads at once, equal to the unpacked one;
   the two backends' folder listings the same; paths spelled as the game spells them; any case where it is asked for; a
   failed read of the .nup said as one (hos_romfs_head: -1).
   Built with host/zs.c as it is (HOS_ZS_ALONE: its decoders, as the game's) and run by builder/crates/game-files
   (romfs.rs, the_host_s_reader), which makes the synthetic .nup, its index and
   the unpacked romfs, and checks that a .nup that is not the index's is refused.

       romfs_test <index> <unpacked romfs folder> <list of the files, one a line> */
#include "../host/romfs.h"
#include "../host/romfs_index.h"
#include "zstd.h"
#include <pthread.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

const char *hos_romfs_dir;        /* (the host's: nn_fs.c) */
HosRomfsIndex *hos_romfs_index;
int MoveFileExA(const char *from, const char *to, unsigned flags);
int MoveFileExA(const char *from, const char *to, unsigned flags) { (void)from, (void)to, (void)flags; return 0; }   /* (hos_zs_write's: not called) */
void hos_log(const char *fmt, ...);
void hos_log(const char *fmt, ...) { (void)fmt; }

static char root[1024];
static char **names;
static int count, failures;

static unsigned char *slurp(const char *path, size_t *n) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    *n = (size_t)ftell(f);
    fseek(f, 0, SEEK_SET);
    unsigned char *b = (unsigned char *)malloc(*n + 1);
    if (fread(b, 1, *n, f) != *n) free(b), b = NULL;
    fclose(f);
    return b;
}

static void failed(const char *what, const char *name) {
    __atomic_add_fetch(&failures, 1, __ATOMIC_RELAXED);
    fprintf(stderr, "FAIL %s: %s\n", what, name);
}

/* every file read whole, and open in pieces of 7 bytes, checked against the unpacked one */
static void *check_all(void *arg) {
    (void)arg;
    for (int i = 0; i < count; i++) {
        char path[2048];
        snprintf(path, sizeof path, "%s/%s", root, names[i]);
        size_t want_n = 0, whole_n = 0;
        unsigned char *want = slurp(path, &want_n);
        uint64_t size = 0;
        if (hos_romfs_stat(names[i], &size) != 1 || size != want_n) { failed("stat", names[i]); free(want); continue; }
        unsigned char *whole = hos_romfs_read(names[i], &whole_n, 1u << 30);
        if (!whole || whole_n != want_n || memcmp(whole, want, want_n)) failed("read", names[i]);
        free(whole);
        HosRomfsFile *f = hos_romfs_open(names[i]);
        unsigned char *got = (unsigned char *)malloc(want_n + 1);
        size_t done = 0;
        while (f && done < want_n) {
            const int64_t n = hos_romfs_file_read(f, done, got + done, 7);
            if (n <= 0) break;
            done += (size_t)n;
        }
        if (!f || hos_romfs_file_size(f) != want_n || hos_romfs_file_read(f, want_n, got, 1) != 0) failed("open", names[i]);
        if (done != want_n || memcmp(got, want, want_n)) failed("pieces", names[i]);
        hos_romfs_file_close(f);
        free(got);
        free(want);
    }
    return NULL;
}

/* a folder's listing, sorted, as one string "a b/ c " (folders with a /) */
typedef struct { char *lines[256]; int n; } Listing;
static void collect(void *arg, const char *name, int is_dir, uint64_t size) {
    (void)size;
    Listing *l = (Listing *)arg;
    if (!l || l->n >= 256) return;
    l->lines[l->n] = (char *)malloc(strlen(name) + 2);
    sprintf(l->lines[l->n++], "%s%s", name, is_dir ? "/" : "");
}
static int by_name(const void *a, const void *b) { return strcmp(*(char *const *)a, *(char *const *)b); }
static char *listing(const char *rel) {
    Listing l = {{0}, 0};
    if (hos_romfs_list(rel, collect, &l) < 0) return NULL;
    qsort(l.lines, (size_t)l.n, sizeof *l.lines, by_name);
    char *out = (char *)calloc(1, 4096);
    for (int i = 0; i < l.n; i++) {
        if (strlen(out) + strlen(l.lines[i]) + 2 < 4096) strcat(strcat(out, l.lines[i]), " ");
        free(l.lines[i]);
    }
    return out;
}

static void pass(const char *mode) {
    pthread_t t[4];
    for (int i = 0; i < 4; i++) pthread_create(&t[i], NULL, check_all, NULL);
    for (int i = 0; i < 4; i++) pthread_join(t[i], NULL);
    if (hos_romfs_stat("/Pack\\\\ZsDic.pack.zs", NULL) != 1) failed("spelling", mode);   /* (the game's: \, //, a leading /) */
    if (hos_romfs_stat("Pack", NULL) != 2 || hos_romfs_stat("", NULL) != 2) failed("folder", mode);
    if (hos_romfs_stat("Pac", NULL) != 0 || hos_romfs_stat("Pack/ZsDic", NULL) != 0 || hos_romfs_open("Pack")) failed("no such", mode);
    char spelled[256];
    if (!hos_romfs_case("pack/ZSDIC.PACK.zs", spelled, sizeof spelled) || strcmp(spelled, "Pack/ZsDic.pack.zs")) failed("case", mode);
    if (hos_romfs_case("pack/none.zs", spelled, sizeof spelled) || strcmp(spelled, "pack/none.zs")) failed("case of none", mode);
    if (hos_romfs_list("big.bin", collect, NULL) != -1) failed("list a file", mode);
    /* a folder in any case (a mod's ui/ where the game has UI/), and of two names that differ in case only, the very one */
    if (!hos_romfs_case("pack", spelled, sizeof spelled) || strcmp(spelled, "Pack")) failed("case of a folder", mode);
    if (!hos_romfs_case("SYSTEM/resource", spelled, sizeof spelled) || strcmp(spelled, "System/Resource")) failed("case of a folder", mode);
    if (!hos_romfs_case("Case/same.txt", spelled, sizeof spelled) || strcmp(spelled, "Case/same.txt")) failed("the very spelling", mode);
    if (!hos_romfs_case("Case/Same.txt", spelled, sizeof spelled) || strcmp(spelled, "Case/Same.txt")) failed("the very spelling", mode);
    if (!hos_romfs_ready()) failed("ready", mode);
}

int main(int argc, char **argv) {
    char why[512];
    if (argc == 3 && !strcmp(argv[1], "bad")) {   /* (a damaged index: refused, said as romfs_index_bad) */
        HosRomfsIndex *ix = hos_romfs_index_open(argv[2], why, sizeof why);
        if (ix) hos_romfs_index_close(ix);
        printf("%s\n", ix ? "opened" : why);
        return !ix && !strncmp(why, "romfs_index_bad:", 16) ? 0 : 1;
    }
    if (argc != 4) return 2;
    snprintf(root, sizeof root, "%s", argv[2]);
    FILE *list = fopen(argv[3], "r");
    char line[1024];
    while (list && fgets(line, sizeof line, list)) {
        line[strcspn(line, "\r\n")] = 0;
        names = (char **)realloc(names, sizeof *names * (size_t)(count + 1));
        names[count++] = strdup(line);
    }
    if (list) fclose(list);
    char beside[64], spelled_late[256];   /* (the index beside image.bin: either separator, the last one; a path that does not fit refused) */
    if (!hos_romfs_index_beside("C:\\Games\\Oot/x\\image.bin", beside, sizeof beside) || strcmp(beside, "C:\\Games\\Oot/x\\longshot_romfs.idx") ||
        !hos_romfs_index_beside("/g/image.bin", beside, sizeof beside) || strcmp(beside, "/g/longshot_romfs.idx") ||
        !hos_romfs_index_beside("image.bin", beside, sizeof beside) || strcmp(beside, "longshot_romfs.idx") ||
        hos_romfs_index_beside("/a/very/long/path/that/does/not/fit/in/sixty-four/bytes/image.bin", beside, sizeof beside))
        failed("the index beside the image", beside);
    if (!hos_romfs_dir_beside("C:\\Games\\Oot/x\\image.bin", beside, sizeof beside) || strcmp(beside, "C:\\Games\\Oot/x\\romfs") ||
        !hos_romfs_dir_beside("image.bin", beside, sizeof beside) || strcmp(beside, "romfs"))
        failed("the romfs folder beside the image", beside);
    hos_romfs_dir = "/no/such/romfs";   /* (the game's, beside image.bin: no folder there) */
    if (hos_romfs_ready()) failed("ready", "no index and no folder");
    hos_romfs_index = hos_romfs_index_open(argv[1], why, sizeof why);
    if (!hos_romfs_index) { fprintf(stderr, "open: %s\n", why); return 1; }
    /* a zstd-stored file's first bytes, from its frame's start; then opened twice: decoded once, kept between */
    unsigned char head[8];
    uint64_t big = 0;
    if (hos_romfs_head("big.bin", head, sizeof head, &big) != 8 || hos_romfs_decoded_bytes()) failed("a head without a whole decode", "big.bin");
    hos_romfs_file_close(hos_romfs_open("big.bin"));
    if (hos_romfs_decoded_bytes() != big) failed("a closed copy kept", "big.bin");
    /* a frame whose window passes 128 MB (zstd's streaming default): its head still read */
    uint64_t wide = 0;
    const uint64_t kept = hos_romfs_decoded_bytes();
    if (hos_romfs_head("Pack/Wide.bin", head, sizeof head, &wide) != 8 || hos_romfs_decoded_bytes() != kept) failed("a head of a frame with a wide window", "Pack/Wide.bin");
    pass("index");
    const char *folders[] = {"", "Pack", "Model", "System/Resource"};
    char *from_index[4];
    for (int i = 0; i < 4; i++) from_index[i] = listing(folders[i]);
    /* a read of the .nup that fails (its handle made a folder's, whose reads fail): a head of a file stored as it is
       says -1, unreadable, not a file of 0 bytes (oot_damaged_files.c leaves such a file to the open) */
    struct stat nup_st, st;
    if (stat(hos_romfs_index_nup(hos_romfs_index), &nup_st)) failed("stat the .nup", hos_romfs_index_nup(hos_romfs_index));
    int nup_fd = -1;
    for (int fd = 3; fd < 256 && nup_fd < 0; fd++)
        if (!fstat(fd, &st) && st.st_dev == nup_st.st_dev && st.st_ino == nup_st.st_ino) nup_fd = fd;
    const char *raw = NULL;
    for (int i = 0; i < count && !raw; i++) {
        HosRomfsEntry e;
        if (hos_romfs_index_find(hos_romfs_index, names[i], &e) == 1 && !e.zstd && e.size >= sizeof head) raw = names[i];
    }
    const int saved = nup_fd >= 0 ? dup(nup_fd) : -1, folder = open(root, O_RDONLY | O_DIRECTORY);
    if (!raw || saved < 0 || folder < 0 || dup2(folder, nup_fd) < 0) failed("make the .nup's reads fail", raw ? raw : "no raw file");
    else if (hos_romfs_head(raw, head, sizeof head, NULL) != -1) failed("a failed read said as -1", raw);
    if (saved >= 0) dup2(saved, nup_fd), close(saved);
    if (folder >= 0) close(folder);
    hos_romfs_index_close(hos_romfs_index);
    hos_romfs_index = NULL;
    hos_romfs_dir = root;
    pass("folder");
    for (int i = 0; i < 4; i++) {
        char *from_folder = listing(folders[i]);
        if (!from_index[i] || !from_folder || strcmp(from_index[i], from_folder)) failed("the same listing", folders[i]);
        if (i == 0) printf("root: %s\n", from_folder ? from_folder : "");
        free(from_folder);
        free(from_index[i]);
    }
    /* (a folder of the romfs folder listed once a start, for hos_romfs_case: a file put there after is not found by
       it, though it is there; game/mods.c asks for every file of every mod) */
    char late[2048];
    snprintf(late, sizeof late, "%s/Pack/late.txt", root);
    FILE *lf = fopen(late, "wb");
    if (lf) fclose(lf);
    if (!lf || hos_romfs_stat("Pack/late.txt", NULL) != 1 || hos_romfs_case("pack/LATE.txt", spelled_late, sizeof spelled_late))
        failed("a folder listed once", "Pack/late.txt");
    remove(late);
    if (failures == 0) printf("ok %d files\n", count);
    return failures ? 1 : 0;
}
