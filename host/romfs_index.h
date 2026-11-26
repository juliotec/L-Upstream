/* (longshot) The game's RomFS read in place from the player's .nup, through the index the builder wrote into the game
   folder (longshot_romfs.idx: builder/crates/game-files/src/romfs.rs has the format). The builder no longer unpacks the
   26 GB: each file is read from the .nup at its offset, as it is, or (a file the .nup keeps as a zstd frame) its frame
   read whole and decoded by the caller (romfs.c, when the file is opened).

   The index is read whole at open, checked (every entry within the .nup, the names in order) and never changes after:
   every lookup is a binary search over it, and every read a positional read on the .nup's one handle (pread; on
   Windows an overlapped ReadFile that waits for its own), so any number of threads read at once with no lock. Paths
   are the RomFS's, '/' or '\' between their parts (runs of them one), case as the game spells them.

   Plain C with no host dependency, so tests/romfs_test.c builds it alone. */
#ifndef HOS_ROMFS_INDEX_H
#define HOS_ROMFS_INDEX_H
#include <stddef.h>
#include <stdint.h>
#ifdef _WIN32
#include <wchar.h>
#endif

typedef struct HosRomfsIndex HosRomfsIndex;

/* A file found: where its data is in the .nup (offset, stored bytes), the file's size, and whether the data is a zstd
   frame of it */
typedef struct {
    uint64_t offset, size, stored;
    int zstd;
} HosRomfsEntry;

/* The index at index_path (UTF-8), its .nup opened and checked to be the one it was made from (its size and its TOC's
   SHA-256, from the .nup's header). NULL on failure, with why (stable code first: "nup_moved: ...", "romfs_index_bad: ...") */
HosRomfsIndex *hos_romfs_index_open(const char *index_path, char *why, size_t cap);
void hos_romfs_index_close(HosRomfsIndex *ix);

/* rel: 1 a file (*out filled), 2 a folder, 0 neither */
int hos_romfs_index_find(const HosRomfsIndex *ix, const char *rel, HosRomfsEntry *out);

/* up to n bytes of a file stored as it is, from off: the bytes read (fewer past its end), -1 on a read error or for a
   zstd-stored file */
int64_t hos_romfs_index_read(const HosRomfsIndex *ix, const HosRomfsEntry *e, uint64_t off, void *buf, uint64_t n);

/* a file's data as the .nup stores it: e->stored bytes into buf; 1 when read whole */
int hos_romfs_index_read_stored(const HosRomfsIndex *ix, const HosRomfsEntry *e, void *buf);
/* up to n bytes of it from off (the start of a zstd file's frame, for its first bytes): how many, -1 on a read error */
int64_t hos_romfs_index_read_frame(const HosRomfsIndex *ix, const HosRomfsEntry *e, uint64_t off, void *buf, uint64_t n);

/* the entries of folder rel ("" the root): fn(arg, name, is_dir, size) for each, files and folders, each once; the
   count, or -1 when rel is no folder */
int hos_romfs_index_list(const HosRomfsIndex *ix, const char *rel, void (*fn)(void *arg, const char *name, int is_dir, uint64_t size),
                         void *arg);

/* rel, a file or a folder, found in any case (ASCII letters folded, as Windows folds file names), its very spelling first:
   its spelling in the index into out; 0 none */
int hos_romfs_index_case(const HosRomfsIndex *ix, const char *rel, char *out, size_t cap);

/* the .nup's path (UTF-8), the files */
const char *hos_romfs_index_nup(const HosRomfsIndex *ix);
uint32_t hos_romfs_index_count(const HosRomfsIndex *ix);

/* ---- the files of the OS, opened read-only and read at an offset (romfs.c reads a romfs folder with them too) */
#ifdef _WIN32
typedef void *RomfsFile;   /* (a HANDLE) */
#define ROMFS_NO_FILE ((RomfsFile)(intptr_t)-1)
int romfs_os_wide(const char *path, wchar_t *w, size_t cap);
#else
typedef int RomfsFile;
#define ROMFS_NO_FILE (-1)
#endif
RomfsFile romfs_os_open(const char *path);
int64_t romfs_os_read(RomfsFile f, uint64_t off, void *buf, uint64_t n);
int64_t romfs_os_size(RomfsFile f);
void romfs_os_close(RomfsFile f);
#endif
