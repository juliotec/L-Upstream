/* (longshot) The RomFS index's reader: romfs_index.h says what it is, builder/crates/game-files/src/romfs.rs has the
   format and writes it. */
#include "romfs_index.h"
#include "romfs.h"   /* (HOS_ROMFS_PATH, hos_romfs_fold: declarations only) */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <fcntl.h>
#include <unistd.h>
#include <sys/stat.h>
#endif

#define HEADER 0x40
#define ENTRY 32
#define PATH_CAP HOS_ROMFS_PATH

struct HosRomfsIndex {
    uint8_t *raw;
    const uint8_t *table;
    const char *names;
    uint32_t count, names_n;
    uint32_t *folded;   /* (the entries by their names in lower case: hos_romfs_index_case) */
    char nup[PATH_CAP];
    RomfsFile file;
};

static uint64_t rd64(const uint8_t *p) {
    uint64_t v = 0;
    for (int i = 7; i >= 0; i--) v = v << 8 | p[i];
    return v;
}
static uint32_t rd32(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }

/* ---- files: opened read-only, read at an offset (no shared cursor: any thread may read) */
#ifdef _WIN32
/* path (UTF-8) as Windows takes it: wide; when long (240 units or more: MAX_PATH less a file name), made whole by
   GetFullPathNameW (a relative path, ".." and "/" resolved, as Windows itself does for a short one) and given its
   extended form, \\?\C:\... or, on a share, \\?\UNC\server\share\... (Windows then takes it as it is). Already
   extended or a device's (\\?\, \\.\): as it is */
int romfs_os_wide(const char *path, wchar_t *w, size_t cap) {
    wchar_t plain[PATH_CAP + 8], full[PATH_CAP + 8];
    const int n = MultiByteToWideChar(CP_UTF8, 0, path, -1, plain, PATH_CAP + 8);
    if (n <= 0) return 0;
    const int special = plain[0] == L'\\' && plain[1] == L'\\' && (plain[2] == L'?' || plain[2] == L'.');
    if (n - 1 < 240 || special) {
        if ((size_t)n > cap) return 0;
        memcpy(w, plain, (size_t)n * sizeof *w);
        return 1;
    }
    const DWORD got = GetFullPathNameW(plain, PATH_CAP + 8, full, NULL);
    if (!got || got >= PATH_CAP + 8) return 0;
    const int unc = full[0] == L'\\' && full[1] == L'\\';
    const wchar_t *prefix = unc ? L"\\\\?\\UNC\\" : L"\\\\?\\";
    const wchar_t *rest = unc ? full + 2 : full;
    if (wcslen(prefix) + wcslen(rest) + 1 > cap) return 0;
    wcscpy(w, prefix);
    wcscat(w, rest);
    return 1;
}
RomfsFile romfs_os_open(const char *path) {
    wchar_t w[PATH_CAP + 8];
    if (!romfs_os_wide(path, w, PATH_CAP + 8)) return ROMFS_NO_FILE;
    /* (overlapped: each read waits for its own, so threads reading one handle at once are not served one at a time. No
       FILE_SHARE_DELETE, on purpose: the index names the .nup by its path, so a .nup moved or renamed is refused at the
       next start (nup_moved) whatever this handle allows. Without it Windows refuses the move while the game runs, and
       says the file is open in Longshot, at the moment the player tries it, rather than the game going on and the next
       start stopping. Linux has no such lock: there the move works and the next start says nup_moved) */
    return CreateFileW(w, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING,
                       FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED, NULL);
}
/* (an overlapped read on a handle others read too waits on an event of its own: one a call, made and closed with it, a
   few microseconds against the read; a thread's own kept one would outlive its thread) */
int64_t romfs_os_read(RomfsFile f, uint64_t off, void *buf, uint64_t n) {
    HANDLE ev = CreateEventW(NULL, TRUE, FALSE, NULL);
    if (!ev) return -1;
    uint64_t done = 0;
    int64_t result = 0;
    while (done < n) {
        OVERLAPPED ov = {0};
        const uint64_t o = off + done;
        ov.Offset = (DWORD)o, ov.OffsetHigh = (DWORD)(o >> 32);
        ov.hEvent = ev;
        DWORD got = 0, want = (DWORD)(n - done > 0x40000000 ? 0x40000000 : n - done);
        if (!ReadFile(f, (uint8_t *)buf + done, want, &got, &ov) &&
            (GetLastError() != ERROR_IO_PENDING || !GetOverlappedResult(f, &ov, &got, TRUE))) {
            if (GetLastError() != ERROR_HANDLE_EOF) result = -1;
            break;
        }
        if (!got) break;
        done += got;
    }
    CloseHandle(ev);
    return result < 0 ? -1 : (int64_t)done;
}
int64_t romfs_os_size(RomfsFile f) {
    LARGE_INTEGER s;
    return GetFileSizeEx(f, &s) ? (int64_t)s.QuadPart : -1;
}
void romfs_os_close(RomfsFile f) { CloseHandle(f); }
#else
RomfsFile romfs_os_open(const char *path) { return open(path, O_RDONLY | O_CLOEXEC); }
int64_t romfs_os_read(RomfsFile f, uint64_t off, void *buf, uint64_t n) {
    uint64_t done = 0;
    while (done < n) {
        const ssize_t got = pread(f, (uint8_t *)buf + done, (size_t)(n - done > 0x40000000 ? 0x40000000 : n - done), (off_t)(off + done));
        if (got < 0) return -1;
        if (!got) break;
        done += (uint64_t)got;
    }
    return (int64_t)done;
}
int64_t romfs_os_size(RomfsFile f) {
    struct stat st;
    return fstat(f, &st) ? -1 : (int64_t)st.st_size;
}
void romfs_os_close(RomfsFile f) { close(f); }
#endif

static int fail(char *why, size_t cap, const char *text, const char *path) {
    if (why && cap) snprintf(why, cap, text, path);
    return 0;
}

static const char *name_of(const HosRomfsIndex *ix, uint32_t k, size_t *len);
static int compare(const char *a, size_t an, const char *b, size_t bn);

/* a and b compared as the same letters in either case (ASCII: what Windows' file names fold) */
static int compare_folded(const char *a, size_t an, const char *b, size_t bn) {
    for (size_t i = 0; i < an && i < bn; i++) {
        const int x = hos_romfs_fold((unsigned char)a[i]), y = hos_romfs_fold((unsigned char)b[i]);
        if (x != y) return x - y;
    }
    return (an > bn) - (an < bn);
}

/* Every entry within the index and the .nup (of nup_size bytes), of a sane size, the names in strict order (a lookup
   is a binary search): what a damaged or a hand-made index would break */
static const HosRomfsIndex *sorting;   /* (qsort has no context argument: the index being checked or folded, at open only) */
static int by_offset(const void *a, const void *b) {
    const uint64_t x = rd64(sorting->table + (size_t)*(const uint32_t *)a * ENTRY), y = rd64(sorting->table + (size_t)*(const uint32_t *)b * ENTRY);
    return (x > y) - (x < y);
}

static int entries_sane(const HosRomfsIndex *ix, uint64_t nup_size) {
    const char *prev = NULL;
    size_t prev_n = 0;
    for (uint32_t k = 0; k < ix->count; k++) {
        const uint8_t *e = ix->table + (size_t)k * ENTRY;
        const uint64_t off = rd64(e), size = rd64(e + 8), stored = rd64(e + 16);
        const size_t name_n = (size_t)(e[28] | e[29] << 8);
        if ((uint64_t)rd32(e + 24) + name_n > ix->names_n || !name_n || e[30] > 1 || e[31]) return 0;
        /* (a raw file is its bytes; a zstd one is decoded whole in memory, at most 1 GB, its frame at most a little
           larger than its content: raw blocks of 128 KB take 3 bytes each, the header 18) */
        const uint64_t extent = e[30] ? stored : size;
        if (e[30] && (size > (1ull << 30) || stored > size + (size >> 15) + 64)) return 0;
        if (extent > nup_size || off > nup_size - extent) return 0;
        const char *name = ix->names + rd32(e + 24);
        if (prev && compare(prev, prev_n, name, name_n) >= 0) return 0;
        prev = name, prev_n = name_n;
    }
    /* (entries at one offset are one file's: a stand-in shares its frame whole. Two of other sizes, lengths or codecs
       there would read one's bytes as the other's. Empty files read nothing and may be anywhere: left out of the sort,
       so one between two others at their offset hides no pair. builder/crates/game-files/src/romfs.rs's `refused` makes
       these very checks at build time) */
    uint32_t *at = (uint32_t *)malloc(sizeof *at * (ix->count ? ix->count : 1));
    if (!at) return 0;
    uint32_t sized = 0;
    for (uint32_t k = 0; k < ix->count; k++)
        if (rd64(ix->table + (size_t)k * ENTRY + 8)) at[sized++] = k;
    sorting = ix;
    qsort(at, sized, sizeof *at, by_offset);
    int ok = 1;
    for (uint32_t k = 1; ok && k < sized; k++) {
        const uint8_t *a = ix->table + (size_t)at[k - 1] * ENTRY, *b = ix->table + (size_t)at[k] * ENTRY;
        if (rd64(a) == rd64(b) && (memcmp(a + 8, b + 8, 16) || a[30] != b[30])) ok = 0;
    }
    free(at);
    return ok;
}

static int by_folded(const void *a, const void *b) {
    size_t an, bn;
    const char *x = name_of(sorting, *(const uint32_t *)a, &an), *y = name_of(sorting, *(const uint32_t *)b, &bn);
    const int c = compare_folded(x, an, y, bn);
    return c ? c : (*(const uint32_t *)a > *(const uint32_t *)b) - (*(const uint32_t *)a < *(const uint32_t *)b);
}

/* the entries in folded order (open is called once, by one thread: hos_main.c, a test) */
static int fold(HosRomfsIndex *ix) {
    ix->folded = (uint32_t *)malloc(sizeof *ix->folded * (ix->count ? ix->count : 1));
    if (!ix->folded) return 0;
    for (uint32_t k = 0; k < ix->count; k++) ix->folded[k] = k;
    sorting = ix;
    qsort(ix->folded, ix->count, sizeof *ix->folded, by_folded);
    sorting = NULL;
    return 1;
}

HosRomfsIndex *hos_romfs_index_open(const char *index_path, char *why, size_t cap) {
    HosRomfsIndex *ix = (HosRomfsIndex *)calloc(1, sizeof *ix);
    if (!ix) return NULL;
    ix->file = ROMFS_NO_FILE;
    RomfsFile f = romfs_os_open(index_path);
    const int64_t n = f == ROMFS_NO_FILE ? -1 : romfs_os_size(f);
    ix->raw = n >= HEADER && n < (1ll << 31) ? (uint8_t *)malloc((size_t)n) : NULL;
    const int whole = ix->raw && romfs_os_read(f, 0, ix->raw, (uint64_t)n) == n;
    if (f != ROMFS_NO_FILE) romfs_os_close(f);
    const uint8_t *r = ix->raw;
    if (!whole || memcmp(r, "LSRI", 4) || rd32(r + 4) != 1) {
        fail(why, cap, "romfs_index_bad: %s is not a game files index of this version: build the game again", index_path);
        goto bad;
    }
    ix->count = rd32(r + 8);
    const uint32_t path_n = rd32(r + 0x38);
    ix->names_n = rd32(r + 0x3c);
    const uint64_t table_at = (HEADER + (uint64_t)path_n + 7) / 8 * 8, names_at = table_at + (uint64_t)ix->count * ENTRY;
    if (path_n >= PATH_CAP || names_at + ix->names_n != (uint64_t)n) {
        fail(why, cap, "romfs_index_bad: %s is damaged: build the game again", index_path);
        goto bad;
    }
    memcpy(ix->nup, r + HEADER, path_n);
    ix->nup[path_n] = 0;
    ix->table = r + table_at;
    ix->names = (const char *)r + names_at;
    if (!entries_sane(ix, rd64(r + 0x10))) {
        fail(why, cap, "romfs_index_bad: %s is damaged: build the game again", index_path);
        goto bad;
    }
    if (!fold(ix)) {
        fail(why, cap, "romfs_index_bad: %s: no memory for its table", index_path);
        goto bad;
    }
    /* (the .nup it was made from: its size and its TOC's SHA-256, which its header holds at +0x18) */
    ix->file = romfs_os_open(ix->nup);
    if (ix->file == ROMFS_NO_FILE) {
        fail(why, cap, "nup_moved: the game reads its files from your .nup %s, which is not there: build the game again from the .nup where it is now", ix->nup);
        goto bad;
    }
    uint8_t head[0x80];
    if (romfs_os_size(ix->file) != (int64_t)rd64(r + 0x10) || romfs_os_read(ix->file, 0, head, sizeof head) != (int64_t)sizeof head ||
        memcmp(head, "NUP\x01", 4) || memcmp(head + 0x18, r + 0x18, 32)) {
        fail(why, cap, "nup_moved: the game reads its files from your .nup %s, which is not the one it was built from: build the game again from your .nup", ix->nup);
        goto bad;
    }
    return ix;
bad:
    hos_romfs_index_close(ix);
    return NULL;
}

void hos_romfs_index_close(HosRomfsIndex *ix) {
    if (!ix) return;
    if (ix->file != ROMFS_NO_FILE) romfs_os_close(ix->file);
    free(ix->folded);
    free(ix->raw);
    free(ix);
}

const char *hos_romfs_index_nup(const HosRomfsIndex *ix) { return ix->nup; }
uint32_t hos_romfs_index_count(const HosRomfsIndex *ix) { return ix->count; }

/* rel as the index spells it: '/' between parts, no leading, trailing or doubled separator. Its length, or -1 */
static int normal(const char *rel, char *out) {
    size_t n = 0;
    for (const char *p = rel; *p; p++) {
        const char c = *p == '\\' ? '/' : *p;
        if (c == '/' && (n == 0 || out[n - 1] == '/')) continue;
        if (n + 1 >= PATH_CAP) return -1;
        out[n++] = c;
    }
    while (n && out[n - 1] == '/') n--;
    out[n] = 0;
    return (int)n;
}

static const char *name_of(const HosRomfsIndex *ix, uint32_t k, size_t *len) {
    const uint8_t *e = ix->table + (size_t)k * ENTRY;
    *len = (size_t)(e[28] | e[29] << 8);
    return ix->names + rd32(e + 24);
}

static int compare(const char *a, size_t an, const char *b, size_t bn) {
    const int c = memcmp(a, b, an < bn ? an : bn);
    return c ? c : (an > bn) - (an < bn);
}

/* the first entry whose name is not below key */
static uint32_t lower_bound(const HosRomfsIndex *ix, const char *key, size_t kn) {
    uint32_t lo = 0, hi = ix->count;
    while (lo < hi) {
        const uint32_t mid = lo + (hi - lo) / 2;
        size_t n;
        const char *name = name_of(ix, mid, &n);
        if (compare(name, n, key, kn) < 0) lo = mid + 1; else hi = mid;
    }
    return lo;
}

static int starts(const HosRomfsIndex *ix, uint32_t k, const char *prefix, size_t pn) {
    size_t n;
    const char *name = k < ix->count ? name_of(ix, k, &n) : NULL;
    return name && n >= pn && !memcmp(name, prefix, pn);
}

int hos_romfs_index_find(const HosRomfsIndex *ix, const char *rel, HosRomfsEntry *out) {
    char key[PATH_CAP + 1];
    const int kn = normal(rel, key);
    if (kn < 0) return 0;
    if (kn == 0) return 2;
    const uint32_t k = lower_bound(ix, key, (size_t)kn);
    size_t n = 0;
    const char *name = k < ix->count ? name_of(ix, k, &n) : NULL;   /* (n set before it is read) */
    if (name && compare(name, n, key, (size_t)kn) == 0) {
        const uint8_t *e = ix->table + (size_t)k * ENTRY;
        if (out) out->offset = rd64(e), out->size = rd64(e + 8), out->stored = rd64(e + 16), out->zstd = e[30] == 1;
        return 1;
    }
    key[kn] = '/';
    return starts(ix, lower_bound(ix, key, (size_t)kn + 1), key, (size_t)kn + 1) ? 2 : 0;
}

int64_t hos_romfs_index_read(const HosRomfsIndex *ix, const HosRomfsEntry *e, uint64_t off, void *buf, uint64_t n) {
    if (e->zstd) return -1;
    if (off >= e->size) return 0;
    if (n > e->size - off) n = e->size - off;
    return romfs_os_read(ix->file, e->offset + off, buf, n);
}

int hos_romfs_index_read_stored(const HosRomfsIndex *ix, const HosRomfsEntry *e, void *buf) {
    return romfs_os_read(ix->file, e->offset, buf, e->stored) == (int64_t)e->stored;
}

int64_t hos_romfs_index_read_frame(const HosRomfsIndex *ix, const HosRomfsEntry *e, uint64_t off, void *buf, uint64_t n) {
    if (off >= e->stored) return 0;
    return romfs_os_read(ix->file, e->offset + off, buf, n > e->stored - off ? e->stored - off : n);
}

int hos_romfs_index_list(const HosRomfsIndex *ix, const char *rel, void (*fn)(void *arg, const char *name, int is_dir, uint64_t size),
                         void *arg) {
    char prefix[PATH_CAP + 1];
    int pn = normal(rel, prefix);
    if (pn < 0) return -1;
    if (pn) prefix[pn++] = '/';
    uint32_t k = lower_bound(ix, prefix, (size_t)pn);
    if (pn && !starts(ix, k, prefix, (size_t)pn)) return hos_romfs_index_find(ix, rel, NULL) == 2 ? 0 : -1;
    int count = 0;
    char last[PATH_CAP + 1] = "", name[PATH_CAP + 1];
    for (; starts(ix, k, prefix, (size_t)pn); k++) {
        size_t n;
        const char *full = name_of(ix, k, &n);
        const char *rest = full + pn;
        const size_t rn = n - (size_t)pn;
        const char *slash = (const char *)memchr(rest, '/', rn);
        const size_t len = slash ? (size_t)(slash - rest) : rn;
        if (len >= sizeof name) continue;
        memcpy(name, rest, len);
        name[len] = 0;
        if (slash) {   /* (a folder: its files are together, sorted; said once) */
            if (!strcmp(name, last)) continue;
            snprintf(last, sizeof last, "%s", name);
            fn(arg, name, 1, 0);
        } else {
            fn(arg, name, 0, rd64(ix->table + (size_t)k * ENTRY + 8));
        }
        count++;
    }
    return count;
}

/* the first entry in folded order whose name is not below key, folded */
static uint32_t folded_bound(const HosRomfsIndex *ix, const char *key, size_t kn) {
    uint32_t lo = 0, hi = ix->count;
    while (lo < hi) {
        const uint32_t mid = lo + (hi - lo) / 2;
        size_t n;
        const char *name = name_of(ix, ix->folded[mid], &n);
        if (compare_folded(name, n, key, kn) < 0) lo = mid + 1; else hi = mid;
    }
    return lo;
}

int hos_romfs_index_case(const HosRomfsIndex *ix, const char *rel, char *out, size_t cap) {
    char key[PATH_CAP + 2];
    const int kn = normal(rel, key);
    if (kn <= 0 || (size_t)kn >= cap) return 0;
    if (hos_romfs_index_find(ix, key, NULL)) {   /* (the very spelling first: two names may differ in case only) */
        memcpy(out, key, (size_t)kn + 1);
        return 1;
    }
    size_t n = 0;
    uint32_t at = folded_bound(ix, key, (size_t)kn);
    const char *name = at < ix->count ? name_of(ix, ix->folded[at], &n) : NULL;
    if (!name || compare_folded(name, n, key, (size_t)kn)) {
        /* (a folder: the first name under it, folded, gives the folder's spelling) */
        key[kn] = '/';
        at = folded_bound(ix, key, (size_t)kn + 1);
        name = at < ix->count ? name_of(ix, ix->folded[at], &n) : NULL;
        if (!name || n <= (size_t)kn || compare_folded(name, (size_t)kn + 1, key, (size_t)kn + 1)) return 0;
        n = (size_t)kn;
    }
    if (n >= cap) return 0;
    memcpy(out, name, n);
    out[n] = 0;
    return 1;
}
