/* (longshot) The game's sounds classed for the controller speaker: the player's own archives read at the start, and the
   class of the asset a wave buffer plays (host/audio_source.h has the formats and the classes' rules).

   hos_audio_source_load reads the archives audio_source_wants names from <romfs>/Sound/Resource/*.bars.zs: each is one
   zstd frame made with the game's dictionary (zs.zsdic, a file of <romfs>/Pack/ZsDic.pack.zs: host/zs.c reads it). Some 40 files, a few megabytes: read once, when the audio renderer opens, and only the checksums
   and classes of their assets are kept. A folder without them (another game, a dump that left them out) has no
   classes, and every sound stays on the game's output.

   HOS_AUDIO_SOURCE=0: nothing is read and no sound is classed. HOS_AUDIO_SOURCE_TRACE=1: each classed sound is logged
   when a voice starts it (checking the classes in the game). */
#include "hos.h"
#include "audio_source.h"
#include "romfs.h"
uint8_t *hos_zs_read_romfs_named(const char *rel, size_t *n, size_t limit, const char *named);   /* zs.c */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#ifndef LONGSHOT_ENV_H   /* (env_on: the switches' one rule) */
#include "../gpu/vk/compat/longshot_env.h"
#endif


enum { AS_MAX_FILE = 64 << 20 };   /* (an archive's size, packed or not: the largest wanted one is about 1 MB) */

static AudioSourceTable as_table;
static volatile LONG as_ready;   /* (set last: the table is only read after it) */

const char *hos_audio_source_name(int cls) {
    static const char *const names[AUDIO_SOURCE_CLASSES] = {"none", "Navi's voice", "Navi's sounds", "targeting", "ocarina", "pick-ups",
                                                            "items", "fairies", "interface", "Stone of Agony", "low health",
                                                            "fishing", "bottles"};
    return cls >= 0 && cls < AUDIO_SOURCE_CLASSES ? names[cls] : "?";
}

/* The archives of Sound/Resource the table wants (their names: <archive>.bars.zs) */
typedef struct { char **names; int n; } AsArchives;
static void as_archive(void *arg, const char *name, int is_dir, uint64_t size) {
    (void)size;
    AsArchives *a = (AsArchives *)arg;
    const size_t len = strlen(name);
    char archive[MAX_PATH];
    if (is_dir || len <= 8 || len >= sizeof archive || strcmp(name + len - 8, ".bars.zs")) return;
    memcpy(archive, name, len - 8);
    archive[len - 8] = 0;
    if (!audio_source_wants(archive)) return;
    char **grown = (char **)realloc(a->names, sizeof *grown * (size_t)(a->n + 1));
    if (!grown) return;
    a->names = grown;
    if ((a->names[a->n] = _strdup(name))) a->n++;
}

static int as_on(void) {
    static int on = -1;
    if (on < 0) on = !getenv("HOS_AUDIO_SOURCE") || env_on("HOS_AUDIO_SOURCE");   /* (unset: on; 0, false, off, no: off) */
    return on;
}

void hos_audio_source_load(void) {
    static volatile LONG started;
    if (!as_on() || !hos_romfs_ready() || InterlockedExchange(&started, 1)) return;
    AsArchives found = {NULL, 0};
    hos_romfs_list("Sound/Resource", as_archive, &found);
    int files = 0, failed = 0, per[AUDIO_SOURCE_CLASSES] = {0};
    for (int k = 0; k < found.n; k++) {
        const char *name = found.names[k];
        char archive[MAX_PATH], rel[MAX_PATH + 16];
        const size_t len = strlen(name);
        memcpy(archive, name, len - 8);
        archive[len - 8] = 0;
        snprintf(rel, sizeof rel, "Sound/Resource/%s", name);
        size_t bars_n = 0;
        /* (with the game's dictionary, by the frame's id: zs.c; a frame of no id, zs.zsdic, as this file named it) */
        uint8_t *bars = hos_zs_read_romfs_named(rel, &bars_n, AS_MAX_FILE, "zs.zsdic");
        if (!bars || audio_source_add_bars(&as_table, archive, bars, bars_n) < 0) {
            if (failed++ < 4) hos_log("audio: %s is not read as a sound archive (%s): its sounds stay on the game's output", name,
                                      bars ? "its table" : "not unpacked");
        } else {
            files++;
        }
        free(bars);
        free(found.names[k]);
    }
    free(found.names);
    for (int i = 0; i < AUDIO_SOURCE_SLOTS; i++) per[as_table.cls[i]]++;
    char line[512];
    int at = 0, classes = 0;
    for (int cls = 1; cls < AUDIO_SOURCE_CLASSES; cls++) {
        classes += per[cls] > 0;
        if (at < (int)sizeof line)
            at += snprintf(line + at, sizeof line - (size_t)at, "%s%s %d", cls > 1 ? ", " : "", hos_audio_source_name(cls), per[cls]);
    }
    hos_log("audio: %d sounds classed from %d archives (%s)%s", as_table.count, files, line,
            as_table.full ? "; the table is full, some are left out" : "");
    /* (what a run shows of the archives' dictionary: none read, no class found) */
    hos_log("audio: %d of %d sound classes found, %d of %d sound archives read", classes, AUDIO_SOURCE_CLASSES - 1, files, found.n);
    InterlockedExchange(&as_ready, 1);
}

static DWORD WINAPI as_load_thread(void *unused) {
    (void)unused;
    hos_audio_source_load();
    return 0;
}

void hos_audio_source_load_async(void) {
    static volatile LONG requested;
    if (!hos_romfs_ready()) return;
    if (InterlockedCompareExchange(&requested, 1, 0)) return;
    HANDLE thread = CreateThread(NULL, 0, as_load_thread, NULL, 0, NULL);
    if (thread) CloseHandle(thread);
    else InterlockedExchange(&requested, 0);
}

int hos_audio_source_ready(void) { return InterlockedCompareExchange(&as_ready, 0, 0) != 0; }

int hos_audio_source_trace(void) {
    static int on = -1;
    if (on < 0) on = env_on("HOS_AUDIO_SOURCE_TRACE");
    return on;
}

/* How many of the `before` bytes just in front of `data` the host may read, from `data` back (committed, readable pages) */
static size_t readable_before(uint64_t data, size_t before) {
#ifdef _WIN32
    size_t span_ok = 0;
    while (span_ok < before) {
        MEMORY_BASIC_INFORMATION info;
        const uintptr_t at = (uintptr_t)GP(data - span_ok) - 1;
        if (!VirtualQuery((const void *)at, &info, sizeof info) || info.State != MEM_COMMIT ||
            (info.Protect & (PAGE_NOACCESS | PAGE_GUARD)) || !(info.Protect & (PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY |
                PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY))) break;
        const size_t span = at - (uintptr_t)info.BaseAddress + 1;
        span_ok += span < before - span_ok ? span : before - span_ok;
    }
    return span_ok;
#else
    (void)data;
    return before;
#endif
}

/* What the searches found, by the address of the samples. A wave's channel starts, taken from its header's own table when
   one of its channels is found: its later channels lie behind the first one's samples, up to megabytes from the header,
   and a sound plays from the same memory each time it plays. An entry is trusted only while the same wave's header (its
   magic and checksum) is still in front of it: the memory may hold another wave by then. And (back 0) a place where the
   whole search found no header: a streamed sound's buffers, which come round again in its ring. One caller at a time
   (host/nn_audio.c's audio lock). */
enum { KNOWN = 512 };
static struct { uint64_t data; uint32_t back, crc; } known[KNOWN];
static unsigned known_slot(uint64_t data) { return (unsigned)(((data >> 6) * 0x9E3779B97F4A7C15ull) >> 55) & (KNOWN - 1); }

/* (a header found `back` bytes in front of `data`: every channel's start kept, by its table) */
static void known_channels(uint64_t data, size_t back, uint32_t crc) {
    const uint8_t *h = (const uint8_t *)GP(data - back);
    const unsigned channels = audio_source_u16(h + BWAV_CHANNELS);
    for (unsigned ch = 0; ch < channels; ch++) {
        const uint32_t off = audio_source_u32(h + BWAV_HEADER + ch * BWAV_CHANNEL + BWAV_CHANNEL_DATA);
        if (off < BWAV_ALIGN * 2) continue;
        const uint64_t start = data - back + off;
        const unsigned s = known_slot(start);
        known[s].data = start, known[s].back = off, known[s].crc = crc;
    }
}

/* The class of the asset whose samples start at the guest address `data` (a wave buffer's), with its checksum in *crc.
   -1 when no asset's samples start there (the buffer continues a streamed sound, or is no archive's): the voice keeps
   the class it had. Asked for every buffer the game queues, so it reads little: a place seen before, else the few spots
   right behind a header where a wave's first channel starts (BWAV_FIRST_MAX); a place found by neither is searched as far
   as a later channel's header can be (BWAV_MAX_BEFORE), once: a streamed sound's ring, which has no header, then costs a
   look in the table each time it comes round. */
int hos_audio_source_of(uint64_t data, uint32_t *crc) {
    if (!hos_audio_source_ready() || !data) return -1;
    const unsigned slot = known_slot(data);
    const int no_header = known[slot].data == data && !known[slot].back;
    if (known[slot].data == data && known[slot].back) {   /* (only its header is read: mapped and readable) */
        const uint64_t at = data - known[slot].back;
        if (data >= known[slot].back && hos_mem_region(at, BWAV_HEADER) &&
            (at >> 12 == data >> 12 || readable_before(at + BWAV_HEADER, BWAV_HEADER) == BWAV_HEADER)) {
            const uint8_t *h = (const uint8_t *)GP(at);
            if (audio_source_is_bwav(h, known[slot].back) && audio_source_u32(h + BWAV_CRC) == known[slot].crc) {
                *crc = known[slot].crc;
                return audio_source_get(&as_table, *crc);
            }
        }
        known[slot].data = 0;
    }
    /* (a wave's first channel: right behind its header; the samples' own page is readable, the game just queued them) */
    size_t before = BWAV_FIRST_MAX;
    while (before >= BWAV_ALIGN * 2 && (data < before || !hos_mem_region(data - before, before))) before -= BWAV_ALIGN;
    if (before >= BWAV_ALIGN * 2 && (data - before) >> 12 != data >> 12) before = readable_before(data, before);
    size_t back = before >= BWAV_ALIGN * 2 ? audio_source_find_back((const uint8_t *)GP(data), before, crc) : 0;
    if (!back && !no_header) {   /* (a later channel's header, as far as it can be) */
        before = BWAV_MAX_BEFORE;
        while (before >= BWAV_ALIGN * 2 && (data < before || !hos_mem_region(data - before, before))) before >>= 1;
        if (before >= BWAV_ALIGN * 2) before = readable_before(data, before);
        if (before >= BWAV_ALIGN * 2) back = audio_source_find_back((const uint8_t *)GP(data), before, crc);
        if (!back) known[slot].data = data, known[slot].back = 0;   /* (none here: not searched that far again) */
    }
    if (!back) return -1;
    known_channels(data, back, *crc);
    return audio_source_get(&as_table, *crc);
}
