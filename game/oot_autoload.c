/* (longshot) The Insert menu's Skip title screen row (gpu/vk/vk_menu.cpp): start with the most recent save instead of the
   title screen.

   The remake has one save the player chooses (its slot_NN folders are the game's own autosave slots). The title screen and
   its file list need a press of A to carry on with that save, so the host does not choose a slot: the game loads its save.
   The newest progress.sav is looked up to know that there is one (a first launch is left on the title screen, where a new
   game can start) and is logged.

   The presses are the host's, in host/nn_hid.c's input script syntax (hos_hid_tap), added to whatever the player and a
   test's script do, and they stop as soon as the player is running (game/oot_tools.c: the lifted player's movement) or after
   120 s. When they come:

   - The hook path (the supported build): the game says when it waits. Its title sequence waits for a button of the title
     screen (Title_00), whose first button is the way on to the saves when there is one; then the file list waits
     (SystemLoadList_00, open on the newest save), and then its check (SystemSaveLoad_00), whose Yes ends in the game's own
     load of that save.
     Each wait is a sequence node, and the sites menu.buttonWait and menu.buttonWaitNoDecideAnim (game/sites/sites.toml,
     game/oot_sites.c) tell its screen as it begins. On those three screens alone A is pressed, 300 ms after the wait
     begins (its screen's opening), again every 600 ms for the first 4 and every 2 s after, until the next wait begins; a
     wait on any other screen gets nothing, and a screen waiting a 4th time (a press that leads back) ends the presses.
     Once the check has had its A and a load begins after it (Sequence_IsLoadingBanc, game/oot_tools.c hos_oot_loading:
     seen down since the press, then up once the A is released), no more: the press was taken, and more would land on
     the load and the spawn. A flag already up at the press tells nothing of it (it can be: kokiri-start's run), so the
     presses go on. The game still chooses: the press is the
     player's, on what the game shows.
   - The timed path: on another build, whose table does not have the sites, A from 20 s after the game's first controller
     read (the intro and title), every 2 s, blind to what the game shows. On the supported build, when no wait on those
     three screens was told by 45 s (none at all, or only others'), the same every 2 s from then, until one is told: then
     the hook path again.

   Each path is logged, and each wait and press with the frame it came at (hos_present_count), so a log tells the time to
   play of either. */
#include "oot_autoload.h"
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include <string.h>
#include <sys/stat.h>
#ifdef _WIN32
#include <windows.h>
#define SEP_S "\\"
#else
#include <dirent.h>
#define SEP_S "/"
#ifndef MAX_PATH
#define MAX_PATH 260
#endif
#endif

extern const char *hos_saves_root(void);   /* (host/nn_fs.c: the profile's saves/, else the user folder) */
extern void hos_log(const char *fmt, ...);
extern int64_t hos_now_ns(void);   /* (host/hos_thread.c) */
extern int rx_option(int id, int otherwise);   /* (host/rx_backend.c) */
extern int hos_oot_player_seen(void);   /* (game/oot_tools.c) */
extern int hos_oot_loading(void);       /* (game/oot_tools.c: the game is loading a map) */
extern void hos_hid_tap(const char *presses);   /* (host/nn_hid.c) */
extern int hos_oot_button_wait_sites(void);   /* (game/oot_sites.c: this build has menu.buttonWait's two sites) */
extern volatile int64_t hos_present_count;   /* (host/display.c: the game's presents, the log's frame) */

/* (the timed path: the wait before the first press, the gap between presses, and when to give up: the intro and title take
   about 23 s on the supported build, and preparing the shaders comes before it) */
enum { kFirstMs = 20000, kEveryMs = 2000, kGiveUpMs = 120000 };
/* (the hook path: a press this long after a wait begins, again this often, at most this many a wait; no wait told by
   kHookGraceMs after the first controller read, the timed path; a screen that waits more than kBeginsMost times, no more
   presses: a press that leads back to it is not repeated) */
enum { kSettleMs = 300, kAgainMs = 600, kPressesPerWait = 4, kHookGraceMs = 45000, kBeginsMost = 3 };
/* (a press's A, "0:A/200": held 200 ms from the read after the tap, which is under a frame later; released by then) */
enum { kReleasedMs = 250 };

static uint64_t file_time(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 ? (uint64_t)st.st_mtime : 0;
}

/* A path built a part at a time, cut to fit (the game's folder can be long; nothing here may leave the buffer). */
static void path_part(char *path, size_t size, const char *part) {
    if (!path || !size) return;
    size_t used = strlen(path);
    if (used + 1 >= size) return;
    size_t copy = strlen(part);
    if (copy > size - used - 1) copy = size - used - 1;
    memcpy(path + used, part, copy);
    path[used + copy] = 0;
}
static void path_copy(char *path, size_t size, const char *text) {
    if (!path || !size) return;
    path[0] = 0, path_part(path, size, text);
}

#ifdef _WIN32
/* The newest progress.sav in one profile's slots: the game writes a new save to a slot of its own choosing, so the file's
   mtime is the save's time. */
static uint64_t profile_save(const char *user_dir, const char *profile, char *path, size_t size) {
    char pattern[MAX_PATH];
    path_copy(pattern, sizeof pattern, user_dir);
    path_part(pattern, sizeof pattern, SEP_S "save" SEP_S);
    path_part(pattern, sizeof pattern, profile);
    path_part(pattern, sizeof pattern, SEP_S "slot_*");
    WIN32_FIND_DATAA fd;
    HANDLE h = FindFirstFileA(pattern, &fd);
    if (h == INVALID_HANDLE_VALUE) return 0;
    uint64_t best = 0;
    char best_path[MAX_PATH] = "";
    do {
        if (!(fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) continue;
        char progress[MAX_PATH];
        path_copy(progress, sizeof progress, user_dir);
        path_part(progress, sizeof progress, SEP_S "save" SEP_S);
        path_part(progress, sizeof progress, profile);
        path_part(progress, sizeof progress, SEP_S);
        path_part(progress, sizeof progress, fd.cFileName);
        path_part(progress, sizeof progress, SEP_S "progress.sav");
        const uint64_t time = file_time(progress);
        if (time > best) {
            best = time;
            path_copy(best_path, sizeof best_path, progress);
        }
    } while (FindNextFileA(h, &fd));
    FindClose(h);
    path_copy(path, size, best_path);
    return best;
}

uint64_t hos_oot_newest_save(const char *user_dir, char *path, size_t size) {
    if (!user_dir || !*user_dir) return 0;
    char pattern[MAX_PATH];
    path_copy(pattern, sizeof pattern, user_dir);
    path_part(pattern, sizeof pattern, SEP_S "save" SEP_S "*");
    WIN32_FIND_DATAA fd;
    HANDLE h = FindFirstFileA(pattern, &fd);
    if (h == INVALID_HANDLE_VALUE) return 0;
    uint64_t best = 0;
    char best_path[MAX_PATH] = "";
    do {
        if (!(fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) || fd.cFileName[0] == '.') continue;
        char found[MAX_PATH];
        const uint64_t time = profile_save(user_dir, fd.cFileName, found, sizeof found);
        if (time > best) {
            best = time;
            path_copy(best_path, sizeof best_path, found);
        }
    } while (FindNextFileA(h, &fd));
    FindClose(h);
    path_copy(path, size, best_path);
    return best;
}
#else
/* (Linux: the same, with the POSIX calls the rest of the host uses) */
static uint64_t profile_save(const char *user_dir, const char *profile, char *path, size_t size) {
    char dir[MAX_PATH];
    path_copy(dir, sizeof dir, user_dir);
    path_part(dir, sizeof dir, SEP_S "save" SEP_S);
    path_part(dir, sizeof dir, profile);
    DIR *d = opendir(dir);
    if (!d) return 0;
    uint64_t best = 0;
    char best_path[MAX_PATH] = "";
    struct dirent *e;
    while ((e = readdir(d))) {
        if (strncmp(e->d_name, "slot_", 5)) continue;
        char progress[MAX_PATH];
        path_copy(progress, sizeof progress, dir);
        path_part(progress, sizeof progress, SEP_S);
        path_part(progress, sizeof progress, e->d_name);
        path_part(progress, sizeof progress, SEP_S "progress.sav");
        const uint64_t time = file_time(progress);
        if (time > best) {
            best = time;
            path_copy(best_path, sizeof best_path, progress);
        }
    }
    closedir(d);
    path_copy(path, size, best_path);
    return best;
}

uint64_t hos_oot_newest_save(const char *user_dir, char *path, size_t size) {
    if (!user_dir || !*user_dir) return 0;
    char dir[MAX_PATH];
    path_copy(dir, sizeof dir, user_dir);
    path_part(dir, sizeof dir, SEP_S "save");
    DIR *d = opendir(dir);
    if (!d) return 0;
    uint64_t best = 0;
    char best_path[MAX_PATH] = "";
    struct dirent *e;
    while ((e = readdir(d))) {
        if (e->d_name[0] == '.') continue;
        char found[MAX_PATH];
        const uint64_t time = profile_save(user_dir, e->d_name, found, sizeof found);
        if (time > best) {
            best = time;
            path_copy(best_path, sizeof best_path, found);
        }
    }
    closedir(d);
    path_copy(path, size, best_path);
    return best;
}
#endif

/* The screens whose button waits get A on the hook path, in the order the game shows them (the file's comment has why) */
static const char *const press_on[] = {"Title_00", "SystemLoadList_00", "SystemSaveLoad_00"};
enum { kScreens = sizeof press_on / sizeof press_on[0] };

static int started, active, hooked;
static int blind;          /* (the hook path's fallback: no wait on press_on's screens told by kHookGraceMs, the clock's presses) */
static int pressable;      /* (a wait on one of press_on's screens has been told) */
static int checked;        /* (the last screen's wait has had its A) */
static int down_since;     /* (the loading flag seen down since that A's press, or as it was pressed) */
static int64_t released_ms;   /* (when that A is released) */
static int done_pressing;  /* (and a load began after it: no more presses) */
static int64_t start_ms, next_ms;
static char save_path[MAX_PATH];
static uint32_t wait_seen;   /* (the input thread's: the last wait it took, its screen, when it began, its presses, and how
                                many waits each screen has had) */
static int wait_screen, wait_presses, wait_begins[kScreens];
static int64_t wait_begun_ms;

/* (the game's threads, at the sites, to the input thread: the latest wait's screen (its index in press_on, -1 another) and
   when it began, under a sequence count: a site takes it from even to odd with a compare-and-swap (one site at a time, the
   others wait their turn), writes both, and stores it even again; the input thread copies both between two equal even
   counts, so a screen is never paired with another wait's time, and the waits told so far are the count / 2. finished:
   the run's skip is over, so the sites read nothing more) */
static volatile int told_screen, finished;
static volatile int64_t told_ms;
static uint32_t told_seq;   /* (atomics only) */

void hos_oot_autoload_reset(void) {
    started = active = hooked = 0;
    blind = pressable = checked = down_since = done_pressing = 0;
    released_ms = 0;
    start_ms = next_ms = 0;
    save_path[0] = 0;
    wait_seen = 0, wait_screen = -1, wait_presses = 0, wait_begun_ms = 0;
    memset(wait_begins, 0, sizeof wait_begins);
    __atomic_store_n(&told_seq, 0, __ATOMIC_SEQ_CST);
    __atomic_store_n(&finished, 0, __ATOMIC_SEQ_CST);
}

/* The Insert menu's row */
static int skip_title_on(void) {
    return rx_option(HOS_OPT_SKIP_TITLE, 0);
}

static long long frame(void) { return (long long)__atomic_load_n(&hos_present_count, __ATOMIC_RELAXED); }

static void finish(void) {
    active = 0;
    __atomic_store_n(&finished, 1, __ATOMIC_RELEASE);
}

int hos_oot_autoload_watching(void) {
    return !__atomic_load_n(&finished, __ATOMIC_ACQUIRE) && skip_title_on();
}

void hos_oot_autoload_wait(const char *screen) {
    if (!screen || !hos_oot_autoload_watching()) return;
    int which = -1;
    for (int i = 0; i < kScreens; i++)
        if (!strcmp(screen, press_on[i])) which = i;
    uint32_t s = __atomic_load_n(&told_seq, __ATOMIC_RELAXED);
    while ((s & 1u) || !__atomic_compare_exchange_n(&told_seq, &s, s + 1u, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED))
        s = __atomic_load_n(&told_seq, __ATOMIC_RELAXED);   /* (another site is telling its wait: it is two stores) */
    __atomic_thread_fence(__ATOMIC_RELEASE);   /* (the odd count before the stores) */
    __atomic_store_n(&told_screen, which, __ATOMIC_RELAXED);
    __atomic_store_n(&told_ms, hos_now_ns() / 1000000, __ATOMIC_RELAXED);
    __atomic_store_n(&told_seq, s + 2u, __ATOMIC_RELEASE);
    hos_log("skip title: the game waits for a button of %.40s at frame %lld%s", screen, frame(),
            which < 0 ? ": not pressed there" : "");
}

/* The hook path's press, if one is due now: the latest wait the game told, on one of press_on's screens */
static void hook_input(int64_t now) {
    const uint32_t seq = __atomic_load_n(&told_seq, __ATOMIC_ACQUIRE), count = seq >> 1;
    if (!(seq & 1u) && count != wait_seen) {
        const int screen = __atomic_load_n(&told_screen, __ATOMIC_RELAXED);
        const int64_t begun = __atomic_load_n(&told_ms, __ATOMIC_RELAXED);
        __atomic_thread_fence(__ATOMIC_ACQUIRE);
        if (__atomic_load_n(&told_seq, __ATOMIC_RELAXED) != seq) return;   /* (another being told meanwhile: the next read) */
        wait_seen = count, wait_screen = screen, wait_begun_ms = begun, wait_presses = 0;
        next_ms = 0;   /* (a new wait's first press is kSettleMs after it began, whatever came before) */
        if (screen >= 0 && ++wait_begins[screen] > kBeginsMost) {   /* (a press that leads back: not pressed in a loop) */
            hos_log("skip title: the game waits on %s for the %dth time: no more presses", press_on[screen], wait_begins[screen]);
            finish();
            return;
        }
        if (screen >= 0) {
            pressable = 1;
            if (blind) {
                blind = 0;
                hos_log("skip title: the game waits on %s at frame %lld: pressing as it waits again", press_on[screen], frame());
            }
        }
    }
    if (done_pressing) return;
    if (checked) {   /* (the check's A taken: a load that begins after its press, seen once the A is released) */
        if (!hos_oot_loading())
            down_since = 1;
        else if (down_since && now >= released_ms) {
            done_pressing = 1;
            hos_log("skip title: the game is loading at frame %lld, after the check's A: no more presses", frame());
            return;
        }
    }
    if (!pressable && !blind) {
        if (now - start_ms < kHookGraceMs) return;
        blind = 1;
        next_ms = now;
        hos_log("skip title: the game told no button wait on the title, its list or its check in %.0f s: pressing A every %.0f s "
                "until it does", kHookGraceMs / 1000.0, kEveryMs / 1000.0);
    }
    if (blind) {
        if (now < next_ms) return;
        hos_hid_tap("0:A/200");
        next_ms = now + kEveryMs;
        return;
    }
    if (wait_screen < 0 || now < wait_begun_ms + kSettleMs || now < next_ms) return;
    hos_hid_tap("0:A/200");
    if (!wait_presses) hos_log("skip title: A on %s at frame %lld", press_on[wait_screen], frame());
    wait_presses++;
    if (wait_screen == kScreens - 1)   /* (the flag down as it is pressed counts: a load after it is the press's) */
        checked = 1, down_since = !hos_oot_loading(), released_ms = now + kReleasedMs;
    next_ms = now + (wait_presses < kPressesPerWait ? kAgainMs : kEveryMs);
}

void game_autoload_input(void) {
    const int64_t now = hos_now_ns() / 1000000;
    if (!started) {
        started = 1;
        start_ms = now, next_ms = now + kFirstMs;
        if (!skip_title_on()) {
            finish();
            return;
        }
        if (!hos_oot_newest_save(hos_saves_root(), save_path, sizeof save_path)) {
            hos_log("skip title: no saved game yet: the title screen stays");
            finish();
            return;
        }
        active = 1;
        hooked = hos_oot_button_wait_sites();
        if (hooked)
            hos_log("skip title: %s is the most recent save: pressing A on the title, its file list and its check as the game "
                    "waits on each (menu.buttonWait), from frame %lld", save_path, frame());
        else
            hos_log("skip title: %s is the most recent save: this build has no menu.buttonWait sites: pressing A from %.0f s, "
                    "every %.0f s", save_path, kFirstMs / 1000.0, kEveryMs / 1000.0);
    }
    if (!active) return;
    if (!rx_option(HOS_OPT_SKIP_TITLE, 0)) {
        hos_log("skip title: the row was turned off while waiting");
        finish();
        return;
    }
    if (rx_option(1, 0)) return;   /* (the Insert menu is open: its presses are its own) */
    if (hos_oot_player_seen()) {
        hos_log("skip title: loaded, the player is running at frame %lld, %.1f s after the first controller read", frame(),
                (now - start_ms) / 1000.0);
        finish();
        return;
    }
    if (now - start_ms >= kGiveUpMs) {
        hos_log("skip title: gave up after %.0f s", kGiveUpMs / 1000.0);
        finish();
        return;
    }
    if (hooked) {
        hook_input(now);
        return;
    }
    if (now < next_ms) return;
    hos_hid_tap("0:A/200");
    next_ms = now + kEveryMs;
}
