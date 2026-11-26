/* (longshot) The game's RomFS as the host's own code reads it: romfs.h says what and why. */
#include "romfs.h"
#include "romfs_index.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <pthread.h>
#include <dirent.h>
#include <fcntl.h>
#include <sys/stat.h>
#endif

extern const char *hos_romfs_dir;        /* nn_fs.c: the romfs folder (set by hos_main.c) */
extern HosRomfsIndex *hos_romfs_index;   /* nn_fs.c: the .nup's index, when the game reads it (set by hos_main.c) */
/* one zstd frame (src, n) decoded into dst (cap bytes): the bytes it holds, or (size_t)-1 (zs.c: the host's libzstd) */
size_t hos_zs_frame(void *dst, size_t cap, const void *src, size_t n);
/* the first cap bytes a frame holds, from its start (src, n: enough of it for them): how many, or (size_t)-1 (zs.c) */
size_t hos_zs_frame_head(void *dst, size_t cap, const void *src, size_t n);
void hos_log(const char *fmt, ...);   /* (hos_thread.c) */

#define CAP HOS_ROMFS_PATH
#ifdef _WIN32
#define SEP '\\'
#else
#define SEP '/'
#endif

/* A zstd-stored file of the .nup, decoded: shared by every open of it, and kept a while after its last close (the game
   opens some files again and again), the closed ones under KEEP_CLOSED bytes, the least recently used let go first */
typedef struct Decoded {
    struct Decoded *next;
    uint64_t offset, stored;   /* (its frame's place and length in the .nup, and size: the key) */
    uint8_t *data;
    uint64_t size, used;   /* (used: when it was last closed) */
    int opens;
} Decoded;
#define KEEP_CLOSED (256ull << 20)
static uint64_t closed_bytes, held_bytes, held_most, said_most, closes;

struct HosRomfsFile {
    RomfsFile os;       /* (a file of the folder) */
    HosRomfsEntry e;    /* (a file of the .nup: e.offset in it) */
    int in_nup;
    Decoded *decoded;   /* (a zstd-stored one, decoded at its open) */
    int frame;          /* (a zstd-stored one opened as it is stored: its reads are its frame's, hos_romfs_open_stored) */
    uint64_t size;
};

/* (the decoded files' list: taken at an open and a close of a zstd-stored file only, never by a read) */
static Decoded *decoded;
#ifdef _WIN32
static SRWLOCK decoded_lock = SRWLOCK_INIT;
#define LOCK() AcquireSRWLockExclusive(&decoded_lock)
#define UNLOCK() ReleaseSRWLockExclusive(&decoded_lock)
#else
static pthread_mutex_t decoded_lock = PTHREAD_MUTEX_INITIALIZER;
#define LOCK() pthread_mutex_lock(&decoded_lock)
#define UNLOCK() pthread_mutex_unlock(&decoded_lock)
#endif

/* e's file decoded, or the copy another open made; NULL when it cannot be read or decoded. Decoded outside the lock:
   two first opens at once may both decode, and the first to finish is kept */
/* (under the lock) the copy of e's frame, opened once more; NULL when none. Keyed by the frame's place, its length and
   the size it holds: an index's two entries of one frame are the same file (romfs_index.c refuses others) */
static Decoded *reopen(const HosRomfsEntry *e) {
    for (Decoded *d = decoded; d; d = d->next)
        if (d->offset == e->offset && d->stored == e->stored && d->size == e->size) {
            if (!d->opens++) closed_bytes -= d->size;
            return d;
        }
    return NULL;
}

static Decoded *decode(const HosRomfsEntry *e) {
    LOCK();
    Decoded *had = reopen(e);
    UNLOCK();
    if (had) return had;
    uint8_t *frame = (uint8_t *)malloc(e->stored ? (size_t)e->stored : 1), *data = (uint8_t *)malloc((size_t)e->size + 1);
    const int ok = frame && data && hos_romfs_index_read_stored(hos_romfs_index, e, frame) &&
                   hos_zs_frame(data, (size_t)e->size, frame, (size_t)e->stored) == (size_t)e->size;
    free(frame);
    Decoded *mine = ok ? (Decoded *)malloc(sizeof *mine) : NULL;
    if (!mine) { free(data); return NULL; }
    *mine = (Decoded){NULL, e->offset, e->stored, data, e->size, 0, 1};
    LOCK();
    if ((had = reopen(e))) {
        UNLOCK();
        free(data), free(mine);
        return had;
    }
    mine->next = decoded, decoded = mine;
    held_bytes += e->size;
    const uint64_t most = held_bytes > held_most ? (held_most = held_bytes) : 0;
    const int say = most >= said_most + (64ull << 20);
    if (say) said_most = most;
    UNLOCK();
    if (say) hos_log("romfs: %llu MB of the .nup's zstd-stored files decoded in memory, the most so far", (unsigned long long)(most >> 20));
    return mine;
}

static void release(Decoded *d) {
    Decoded *gone = NULL;   /* (let go of: freed outside the lock) */
    LOCK();
    if (!--d->opens) {
        d->used = ++closes;
        closed_bytes += d->size;
        while (closed_bytes > KEEP_CLOSED) {
            Decoded **oldest = NULL;
            for (Decoded **p = &decoded; *p; p = &(*p)->next)
                if (!(*p)->opens && (!oldest || (*p)->used < (*oldest)->used)) oldest = p;
            Decoded *o = *oldest;
            *oldest = o->next;
            closed_bytes -= o->size, held_bytes -= o->size;
            o->next = gone, gone = o;
        }
    }
    UNLOCK();
    while (gone) {
        Decoded *next = gone->next;
        free(gone->data), free(gone);
        gone = next;
    }
}

/* (a folder of the system's: 1) */
static int os_stat(const char *path, uint64_t *size);

uint64_t hos_romfs_decoded_bytes(void) {
    LOCK();
    const uint64_t n = held_bytes;
    UNLOCK();
    return n;
}

static int beside(const char *image, const char *name, char *out, size_t cap) {
    const char *slash = NULL;
    for (const char *p = image; *p; p++)
        if (*p == '/' || *p == '\\') slash = p;
    const int w = snprintf(out, cap, "%.*s%s", slash ? (int)(slash - image + 1) : 0, image, name);
    return w > 0 && (size_t)w < cap;
}
int hos_romfs_index_beside(const char *image, char *out, size_t cap) { return beside(image, "longshot_romfs.idx", out, cap); }
int hos_romfs_dir_beside(const char *image, char *out, size_t cap) { return beside(image, "romfs", out, cap); }

int hos_romfs_ready(void) { return hos_romfs_index || (hos_romfs_dir && os_stat(hos_romfs_dir, NULL) == 2); }

/* the folder's path for rel: the folder, then each part of rel after the system's separator; 0 when too long */
static int folder_path(const char *rel, char *out, size_t cap) {
    if (!hos_romfs_dir) return 0;
    size_t n = (size_t)snprintf(out, cap, "%s", hos_romfs_dir);
    for (const char *p = rel; n < cap && *p;) {
        while (*p == '/' || *p == '\\') p++;
        const char *end = p;
        while (*end && *end != '/' && *end != '\\') end++;
        if (end > p) n += (size_t)snprintf(out + n, cap - n, "%c%.*s", SEP, (int)(end - p), p);
        p = end;
    }
    return n < cap;
}

/* ---- the folder: what its file system says */
#ifdef _WIN32
static int os_stat(const char *path, uint64_t *size) {
    wchar_t w[CAP + 8];
    WIN32_FILE_ATTRIBUTE_DATA a;
    if (!romfs_os_wide(path, w, CAP + 8) || !GetFileAttributesExW(w, GetFileExInfoStandard, &a)) return 0;
    if (a.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) return 2;
    if (size) *size = (uint64_t)a.nFileSizeHigh << 32 | a.nFileSizeLow;
    return 1;
}
static int os_list(const char *dir, HosRomfsVisit fn, void *arg) {
    char pattern[CAP + 4];
    wchar_t w[CAP + 16];
    if ((size_t)snprintf(pattern, sizeof pattern, "%s\\*", dir) >= sizeof pattern || !romfs_os_wide(pattern, w, CAP + 16)) return -1;
    WIN32_FIND_DATAW fd;
    HANDLE f = FindFirstFileW(w, &fd);
    if (f == INVALID_HANDLE_VALUE) return -1;
    int n = 0;
    do {
        char name[CAP];
        if (!WideCharToMultiByte(CP_UTF8, 0, fd.cFileName, -1, name, sizeof name, NULL, NULL) || !strcmp(name, ".") || !strcmp(name, ".."))
            continue;
        fn(arg, name, (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0, (uint64_t)fd.nFileSizeHigh << 32 | fd.nFileSizeLow);
        n++;
    } while (FindNextFileW(f, &fd));
    FindClose(f);
    return n;
}
#else
static int os_stat(const char *path, uint64_t *size) {
    struct stat st;
    if (stat(path, &st)) return 0;
    if (S_ISDIR(st.st_mode)) return 2;
    if (!S_ISREG(st.st_mode)) return 0;
    if (size) *size = (uint64_t)st.st_size;
    return 1;
}
static int os_list(const char *dir, HosRomfsVisit fn, void *arg) {
    DIR *d = opendir(dir);
    if (!d) return -1;
    int n = 0;
    for (struct dirent *e; (e = readdir(d));) {
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
        struct stat st;
        if (fstatat(dirfd(d), e->d_name, &st, 0)) continue;
        fn(arg, e->d_name, S_ISDIR(st.st_mode), S_ISDIR(st.st_mode) ? 0 : (uint64_t)st.st_size);
        n++;
    }
    closedir(d);
    return n;
}
#endif

int hos_romfs_stat(const char *rel, uint64_t *size) {
    if (hos_romfs_index) {
        HosRomfsEntry e;
        const int kind = hos_romfs_index_find(hos_romfs_index, rel, &e);
        if (kind == 1 && size) *size = e.size;
        return kind;
    }
    char path[CAP];
    return folder_path(rel, path, sizeof path) ? os_stat(path, size) : 0;
}

/* ---- any case, in a folder: each part the folder's own entry of that name, its letters folded (ASCII). Each folder of
   the romfs folder is listed once a start and its names kept, sorted folded (game/mods.c asks for every file of every
   mod: listing a folder each time was its files times the folder's size) */
static int compare_folded(const char *a, size_t an, const char *b, size_t bn) {
    for (size_t i = 0; i < an && i < bn; i++) {
        const int x = hos_romfs_fold((unsigned char)a[i]), y = hos_romfs_fold((unsigned char)b[i]);
        if (x != y) return x - y;
    }
    return (an > bn) - (an < bn);
}
typedef struct { char *dir; char **names; size_t n, cap; } Listing;   /* (names: NULL n 0 for a folder that cannot be listed) */
static Listing *listings;   /* (open addressing by the folder's path, listings_cap a power of two, at most half full) */
static size_t nlistings, listings_cap;
#ifdef _WIN32
static SRWLOCK listings_lock = SRWLOCK_INIT;
#define LIST_LOCK() AcquireSRWLockExclusive(&listings_lock)
#define LIST_UNLOCK() ReleaseSRWLockExclusive(&listings_lock)
#else
static pthread_mutex_t listings_lock = PTHREAD_MUTEX_INITIALIZER;
#define LIST_LOCK() pthread_mutex_lock(&listings_lock)
#define LIST_UNLOCK() pthread_mutex_unlock(&listings_lock)
#endif
static size_t dir_hash(const char *s) {
    size_t h = 1469598103934665603ull & (size_t)-1;
    while (*s) h = (h ^ (unsigned char)*s++) * (size_t)1099511628211ull;
    return h;
}
static char *dup_text(const char *s) {   /* (strdup, which Windows' C library calls _strdup) */
    const size_t n = strlen(s) + 1;
    char *d = (char *)malloc(n);
    return d ? (char *)memcpy(d, s, n) : NULL;
}
static void collect_name(void *arg, const char *name, int is_dir, uint64_t size) {
    (void)is_dir, (void)size;
    Listing *l = (Listing *)arg;
    if (l->n == l->cap) {
        char **grown = (char **)realloc(l->names, sizeof *grown * (l->cap ? l->cap * 2 : 16));
        if (!grown) return;
        l->names = grown, l->cap = l->cap ? l->cap * 2 : 16;
    }
    if ((l->names[l->n] = dup_text(name))) l->n++;
}
static int by_folded_name(const void *a, const void *b) {
    const char *x = *(char *const *)a, *y = *(char *const *)b;
    const int c = compare_folded(x, strlen(x), y, strlen(y));
    return c ? c : strcmp(x, y);
}
/* (under the lock) dir's listing, listed the first time it is asked for; NULL when there is no room for it */
static const Listing *listing_of(const char *dir) {
    if (2 * (nlistings + 1) > listings_cap) {
        const size_t cap = listings_cap ? listings_cap * 2 : 256;
        Listing *grown = (Listing *)calloc(cap, sizeof *grown);
        if (!grown) return NULL;
        for (size_t i = 0; i < listings_cap; i++)
            if (listings[i].dir) {
                size_t k = dir_hash(listings[i].dir) & (cap - 1);
                while (grown[k].dir) k = (k + 1) & (cap - 1);
                grown[k] = listings[i];
            }
        free(listings);
        listings = grown, listings_cap = cap;
    }
    size_t k = dir_hash(dir) & (listings_cap - 1);
    for (; listings[k].dir; k = (k + 1) & (listings_cap - 1))
        if (!strcmp(listings[k].dir, dir)) return &listings[k];
    Listing l = {dup_text(dir), NULL, 0, 0};
    if (!l.dir) return NULL;
    if (os_list(dir, collect_name, &l) < 0) {
        for (size_t i = 0; i < l.n; i++) free(l.names[i]);
        free(l.names);
        l.names = NULL, l.n = 0;
    }
    if (l.n) qsort(l.names, l.n, sizeof *l.names, by_folded_name);
    listings[k] = l;
    nlistings++;
    return &listings[k];
}
/* the entry of dir named part (n bytes) in any case, into found: the very spelling when there is one, else another;
   0 when none */
static int folder_entry(const char *dir, const char *part, size_t n, char *found, size_t cap) {
    LIST_LOCK();
    const Listing *l = listing_of(dir);
    size_t lo = 0, hi = l ? l->n : 0;
    while (lo < hi) {   /* (the first name not below part, folded) */
        const size_t mid = lo + (hi - lo) / 2;
        if (compare_folded(l->names[mid], strlen(l->names[mid]), part, n) < 0) lo = mid + 1;
        else hi = mid;
    }
    const char *pick = NULL;
    for (size_t i = lo; l && i < l->n && !compare_folded(l->names[i], strlen(l->names[i]), part, n); i++) {
        if (!pick) pick = l->names[i];
        if (strlen(l->names[i]) == n && !memcmp(l->names[i], part, n)) {
            pick = l->names[i];
            break;
        }
    }
    const int ok = pick && strlen(pick) < cap;
    if (ok) memcpy(found, pick, strlen(pick) + 1);
    LIST_UNLOCK();
    return ok;
}

int hos_romfs_case(const char *rel, char *out, size_t cap) {
    snprintf(out, cap, "%s", rel);
    if (hos_romfs_index) return hos_romfs_index_case(hos_romfs_index, rel, out, cap) || (snprintf(out, cap, "%s", rel), 0);
    char dir[CAP], spelled[CAP] = "";
    if (!folder_path("", dir, sizeof dir)) return 0;
    for (const char *p = rel; *p;) {
        while (*p == '/' || *p == '\\') p++;
        if (!*p) break;
        const char *end = p;
        while (*end && *end != '/' && *end != '\\') end++;
        char found[CAP];
        if (!folder_entry(dir, p, (size_t)(end - p), found, sizeof found)) return 0;
        size_t dn = strlen(dir), sn = strlen(spelled);
        const size_t fn = strlen(found);
        if (dn + 1 + fn >= sizeof dir || sn + 1 + fn >= sizeof spelled) return 0;
        dir[dn++] = SEP;
        memcpy(dir + dn, found, fn + 1);
        if (sn) spelled[sn++] = '/';
        memcpy(spelled + sn, found, fn + 1);
        p = end;
    }
    if (!os_stat(dir, NULL) || strlen(spelled) >= cap) return 0;
    snprintf(out, cap, "%s", spelled);
    return 1;
}

int hos_romfs_list(const char *rel, HosRomfsVisit fn, void *arg) {
    if (hos_romfs_index) return hos_romfs_index_list(hos_romfs_index, rel, fn, arg);
    char path[CAP];
    return folder_path(rel, path, sizeof path) ? os_list(path, fn, arg) : -1;
}

static HosRomfsFile *open_file(const char *rel, int stored, int *framed) {
    HosRomfsFile f = {ROMFS_NO_FILE, {0, 0, 0, 0}, 0, NULL, 0, 0};
    char path[CAP];
    if (framed) *framed = 0;
    if (hos_romfs_index) {
        if (hos_romfs_index_find(hos_romfs_index, rel, &f.e) != 1) return NULL;
        f.size = f.e.size, f.in_nup = 1;
        if (f.e.zstd && stored) f.frame = 1, f.size = f.e.stored, *framed = 1;
        else if (f.e.zstd && !(f.decoded = decode(&f.e))) return NULL;
    } else {
        if (!folder_path(rel, path, sizeof path) || os_stat(path, &f.size) != 1 || (f.os = romfs_os_open(path)) == ROMFS_NO_FILE)
            return NULL;
    }
    HosRomfsFile *out = (HosRomfsFile *)malloc(sizeof *out);
    if (out) *out = f;
    else if (f.os != ROMFS_NO_FILE) romfs_os_close(f.os);
    else if (f.decoded) release(f.decoded);
    return out;
}

HosRomfsFile *hos_romfs_open(const char *rel) { return open_file(rel, 0, NULL); }

HosRomfsFile *hos_romfs_open_stored(const char *rel, int *framed) { return open_file(rel, 1, framed); }

uint64_t hos_romfs_file_size(const HosRomfsFile *f) { return f->size; }

int64_t hos_romfs_file_read(HosRomfsFile *f, uint64_t off, void *buf, uint64_t n) {
    if (off >= f->size) return 0;
    if (f->decoded) {   /* (within the copy too, whatever the handle's size) */
        if (off >= f->decoded->size) return 0;
        if (n > f->size - off) n = f->size - off;
        if (n > f->decoded->size - off) n = f->decoded->size - off;
        memcpy(buf, f->decoded->data + off, (size_t)n);
        return (int64_t)n;
    }
    if (f->frame) return hos_romfs_index_read_frame(hos_romfs_index, &f->e, off, buf, n);
    if (f->in_nup) return hos_romfs_index_read(hos_romfs_index, &f->e, off, buf, n);
    return romfs_os_read(f->os, off, buf, n > f->size - off ? f->size - off : n);
}

void hos_romfs_file_close(HosRomfsFile *f) {
    if (!f) return;
    if (f->os != ROMFS_NO_FILE) romfs_os_close(f->os);
    if (f->decoded) release(f->decoded);
    free(f);
}

uint8_t *hos_romfs_read(const char *rel, size_t *n, size_t limit) {
    HosRomfsFile *f = hos_romfs_open(rel);
    if (!f) return NULL;
    uint8_t *b = f->size <= limit ? (uint8_t *)malloc((size_t)f->size + 1) : NULL;
    if (b && hos_romfs_file_read(f, 0, b, f->size) != (int64_t)f->size) free(b), b = NULL;
    if (b) b[f->size] = 0, *n = (size_t)f->size;
    hos_romfs_file_close(f);
    return b;
}

int64_t hos_romfs_head(const char *rel, void *buf, size_t n, uint64_t *size) {
    HosRomfsEntry e;
    if (hos_romfs_index && hos_romfs_index_find(hos_romfs_index, rel, &e) == 1 && e.zstd) {
        /* (a zstd-stored file: its first bytes from its frame's first blocks, not the whole file decoded; a copy that
           is open or kept serves them as it is) */
        if (size) *size = e.size;
        LOCK();
        Decoded *had = reopen(&e);
        UNLOCK();
        if (had) {
            const size_t k = n < had->size ? n : (size_t)had->size;
            memcpy(buf, had->data, k);
            release(had);
            return (int64_t)k;
        }
        if (n > e.size) n = (size_t)e.size;
        uint8_t *frame = (uint8_t *)malloc(256u << 10);
        const int64_t got = frame ? hos_romfs_index_read_frame(hos_romfs_index, &e, 0, frame, 256u << 10) : -1;
        const size_t k = got > 0 ? hos_zs_frame_head(buf, n, frame, (size_t)got) : (size_t)-1;
        free(frame);
        if (k != (size_t)-1) return (int64_t)k;
    }
    HosRomfsFile *f = hos_romfs_open(rel);
    if (!f) return -1;
    if (size) *size = f->size;
    const int64_t got = n ? hos_romfs_file_read(f, 0, buf, n) : 0;
    hos_romfs_file_close(f);
    return got;   /* (-1: it could not be read, which is no file of 0 bytes: oot_damaged_files.c leaves it to the open) */
}
