/* (longshot) The game's RomFS as the host's own code reads it: prompts, mods, the sound sources, the heart containers,
   the damaged-file stand-ins, the zstd dictionaries, the Vulkan back end's menu font (through rx_backend.c). One API over
   either place a RomFS is:
     - the player's .nup, through the builder's index (romfs_index.h), when the game folder has one and no romfs folder;
     - a romfs folder (hos_romfs_dir: --romfs, or romfs beside the executable), as a developer's unpacked dump is.
   Paths are the RomFS's ("Pack/ZsDic.pack.zs"), '/' or '\' between parts. A name is found as the place finds it: the
   index exactly as the game spells it, a folder as its file system does (Windows any case, Linux exactly);
   hos_romfs_case finds it in any case where a caller needs that (a mod's path in another case than the game's).
   nn_fs.c, the game's own file calls, reads the RomFS through this too, whichever place it is in.
   Every call may come from any thread: the index never changes once open, and a file is read at an offset. A file the
   .nup keeps as a zstd frame is decoded at its open, one copy for every open of it, kept a while after (256 MB of
   closed ones at most); its first bytes alone (hos_romfs_head) come from its frame's first blocks. */
#ifndef HOS_ROMFS_H
#define HOS_ROMFS_H
#include <stddef.h>
#include <stdint.h>

/* The longest RomFS path the host's RomFS code takes, its 0 included (the game's longest is far shorter) */
#define HOS_ROMFS_PATH 1024
/* c's letter folded to lower case, ASCII only: how a RomFS path matches in any case (Windows' file names fold so) */
static inline int hos_romfs_fold(int c) { return c >= 'A' && c <= 'Z' ? c + 32 : c; }

/* the bytes of zstd-stored files decoded in memory now, open or kept */
uint64_t hos_romfs_decoded_bytes(void);

/* the index's path beside image (UTF-8, either separator): 0 when it does not fit in cap */
int hos_romfs_index_beside(const char *image, char *out, size_t cap);
/* the romfs folder's path beside image, as a game started with no image has it beside the executable: 0 when it does
   not fit */
int hos_romfs_dir_beside(const char *image, char *out, size_t cap);

/* whether the game has a RomFS (an index open, or a folder that is there) */
int hos_romfs_ready(void);

/* rel: 1 a file (*size its size, when size is given), 2 a folder, 0 neither */
int hos_romfs_stat(const char *rel, uint64_t *size);

/* rel's spelling in the RomFS, a file or a folder, its letters in any case (its very spelling first): 1 with it in out;
   0 when there is none (out: rel) */
int hos_romfs_case(const char *rel, char *out, size_t cap);

/* a whole file: malloc'd (one byte more, for a caller's 0), *n its size; NULL when there is none, or it is larger than
   limit, or it cannot be read */
uint8_t *hos_romfs_read(const char *rel, size_t *n, size_t limit);

/* the first n bytes (fewer when it is shorter): how many; -1 when there is no such file or it could not be read. *size
   the file's size */
int64_t hos_romfs_head(const char *rel, void *buf, size_t n, uint64_t *size);

/* a folder's entries, each once: fn(arg, name, is_dir, size); their count, or -1 when rel is no folder */
typedef void (*HosRomfsVisit)(void *arg, const char *name, int is_dir, uint64_t size);
int hos_romfs_list(const char *rel, HosRomfsVisit fn, void *arg);

/* a file open for reads at offsets (a stream's, a handle's): NULL when there is none */
typedef struct HosRomfsFile HosRomfsFile;
HosRomfsFile *hos_romfs_open(const char *rel);
/* the same, but a file the .nup keeps as a zstd frame is not decoded: its reads are the frame's (*framed 1), for a caller
   that streams it (zs.c, a .zs file of hundreds of MB the .nup wraps in a frame of its own) */
HosRomfsFile *hos_romfs_open_stored(const char *rel, int *framed);
uint64_t hos_romfs_file_size(const HosRomfsFile *f);
/* up to n bytes from off: how many (0 past the end), -1 on a read error */
int64_t hos_romfs_file_read(HosRomfsFile *f, uint64_t off, void *buf, uint64_t n);
void hos_romfs_file_close(HosRomfsFile *f);
#endif
