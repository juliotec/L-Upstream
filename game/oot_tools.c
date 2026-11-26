/* The game's tools on the host's side, cut out of host/hos_main.c as they were: the Debug tab's cheats (hearts, flight,
   the free camera, speed), its inventory and teleports and the age, the control bridge's game commands (stats, state, the
   map, the destinations, teleport and age requests), the opening's skip, the save trace, Link's place for the
   performance summary, and the map line. The host and the renderer call them by name; they go through game/game.h. */
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include "flags.h"   /* (the game's flags: Link's age, the map, the songs) */
#include "oot_cheat_items.h"
#include "oot_teleport_check.h"   /* (the teleport's follow-up: a dropped request sent again) */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <windows.h>   /* (linux: host/posix's) */
#endif
const char *hos_version(void);   /* (host/version.c) */
#ifndef LONGSHOT_VERSION   /* (a test that cuts a piece of this file out may give its own) */
#define LONGSHOT_VERSION hos_version()
#endif

/* (longshot) the map Link is in and his age, as the last map: line, for the performance summary (gpu/vk/perf_report.cpp
   through MkwVkHost perf_info): "" while no save is loaded (before one, and back at the title) */
static SRWLOCK map_lock = SRWLOCK_INIT;
static char map_now[100];
/* (Link's pose for the same summary's one-second samples: oot_perf_pose sets it from his update, at most every
   PERF_POSE_EVERY_NS, and clears it while an event plays; one not set for PERF_POSE_STALE_NS is no pose, since Link isn't
   updating: the title, the file select, a reset, a load. hos_perf_info gives none while there is no map either, and
   oot_inventory_refresh clears it where the map line changes: a pose is for the map the sample names) */
#define PERF_POSE_EVERY_NS 250000000LL
#define PERF_POSE_STALE_NS 1000000000LL
static SRWLOCK perf_pose_lock = SRWLOCK_INIT;
static float perf_pose_now[5];
static int perf_pose_valid;
static int64_t perf_pose_ns;
static volatile LONG perf_pose_enabled;
static void perf_pose_clear(void) {
    AcquireSRWLockExclusive(&perf_pose_lock);
    perf_pose_valid = 0;
    ReleaseSRWLockExclusive(&perf_pose_lock);
}
void hos_movie_stats(int64_t out[5]);   /* nn_video.c */
const char *hos_perf_info(char *map, int cap, int64_t movie[5], float pose[5], int *has_pose) {
    AcquireSRWLockShared(&map_lock);
    snprintf(map, (size_t)cap, "%s", map_now);
    ReleaseSRWLockShared(&map_lock);
    hos_movie_stats(movie);
    const int64_t now = hos_now_ns();
    AcquireSRWLockShared(&perf_pose_lock);
    *has_pose = cap > 0 && map[0] && perf_pose_valid && now - perf_pose_ns < PERF_POSE_STALE_NS;
    if (*has_pose) memcpy(pose, perf_pose_now, sizeof perf_pose_now);
    ReleaseSRWLockShared(&perf_pose_lock);
    return LONGSHOT_VERSION;
}

void hos_perf_tracking(int enabled) {
    InterlockedExchange(&perf_pose_enabled, enabled != 0);
    if (!enabled) perf_pose_clear();
}

int hos_oot_protect_hearts;

/* The game's own start (game.h), where host/hos_main.c read it before: HOS_OOT_PROTECT_HEARTS=1, Infinite hearts on unless
   the Debug tab says otherwise (hos_oot_hearts_enabled) */
void game_init(void) {
    void hos_oot_heap_test_init(void);
    hos_oot_heap_test_init();
    const char *protect_hearts = getenv("HOS_OOT_PROTECT_HEARTS");
    hos_oot_protect_hearts = protect_hearts && !strcmp(protect_hearts, "1");
}
/* Trace the request before the save manager queues it. */
int hos_oot_trace_save(Ctx *c) {
    int rx_option(int id, int otherwise);
    const uint64_t caller = c->x[30];
    const uint64_t parent = caller == 0x80bc7764ULL ? recomp_load64(c, c->x[31] + 24) : 0;
    if (env_on("HOS_OOT_TRACE_SAVES")) hos_log("save trace: reason=%u mode=%u caller=%#llx request_site=%#llx", (unsigned)c->x[1], (unsigned)c->x[2],
            (unsigned long long)caller, (unsigned long long)(parent ? parent : caller));
    const unsigned reason = (uint32_t)c->x[1];
    /* A crossing can queue both arrival requests in addition to the transition request. */
    const int transition = (parent == 0x80bc7710ULL && reason == 6) ||
        (parent == 0x80bc7864ULL && reason == 2) ||
        (parent == 0x832a9764ULL && reason == 1);
    if (caller == 0x80bc7764ULL && transition &&
        (uint32_t)c->x[2] == 0 && !rx_option(HOS_OPT_TRANSITION_AUTOSAVE, 1)) {
        if (env_on("HOS_OOT_TRACE_SAVES")) hos_log("save trace: transition autosave suppressed");
        c->x[0] = 0;
        return 1;
    }
    return 0;
}

volatile int game_live_action;   /* (test runs) the live input file's "!cheat <action>": host/nn_hid.c */

int hos_oot_hearts_enabled(void) {
    int rx_option(int id, int otherwise);
    const int menu = rx_option(HOS_OPT_CHEAT_HEARTS, -1);
    return menu < 0 ? hos_oot_protect_hearts : menu;
}

int hos_oot_is_player(Ctx *c, uint64_t actor) {
    if (recomp_load64(c, actor) != 0x84b6c460ULL) return 0;
    // Other actors share this vtable. The actor's resource record identifies Link.
    const uint64_t resource = recomp_load64(c, actor + 16);
    return resource && recomp_load32(c, resource + 88) == 0x79616c50 &&
           recomp_load16(c, resource + 92) == 0x7265 &&
           recomp_load8(c, resource + 94) == 0;
}

/* Movement speed (the Debug tab's row): Link's character controller and the row's percent, as oot_player_speed saw them on
   his last update (0: none, or 1x) */
static uint64_t oot_speed_controller;
static int oot_speed_percent = 100;

/* A character controller's forward speed being set (the hook sites player.speed*, game/oot_sites.c: s0, in metres a second, or a frame when
   per_second is 0): Link's scaled by the row while it is above 1x and the player steers him (a scene's walk, as the one into a
   map, keeps its pace), the physics step moving him by it with its collisions. Only speeds up to 15 m/s (his run is 4.5): a ledge
   climb sets some 55 for a frame, and scaled it threw him past the ledge. Every other controller as it is. */
void hos_oot_cct_speed(Ctx *c, uint64_t controller, int per_second) {
    if (!controller || controller != __atomic_load_n(&oot_speed_controller, __ATOMIC_RELAXED)) return;
    const int percent = __atomic_load_n(&oot_speed_percent, __ATOMIC_RELAXED);
    int hos_move_stick_held(void);   /* host/nn_hid.c */
    if (percent <= 100 || percent > 400 || !hos_move_stick_held()) return;
    float speed;
    memcpy(&speed, &c->vreg[0][0], sizeof speed);
    if (!(fabsf(speed) * (per_second ? 1.0f : 60.0f) <= 15.0f)) return;   /* (and NaN) */
    speed *= percent / 100.0f;
    memcpy(&c->vreg[0][0], &speed, sizeof speed);
}

#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
/* OneShotAddOrRemovePouchContent (f_828d4a70) for one pouch content by actor name: delta > 0 adds, < 0 removes.
   Returns the event status (top bits of the result word), or -1 when the pouch is not ready. */
static int oot_pouch_change(Ctx *c, const char *name, int delta) {
    const uint64_t pouch_singleton = recomp_load64(c, 0x84c9e0d0ULL);
    const uint64_t state_singleton = recomp_load64(c, 0x84c9e2a8ULL);
    const uint64_t actor_db = recomp_load64(c, 0x84eb6888ULL);
    const uint64_t actor_info = recomp_load64(c, 0x84e8c060ULL);
    if (!game_has(0x828d4a70ULL) || !pouch_singleton || !recomp_load64(c, pouch_singleton) ||
        !state_singleton || !recomp_load64(c, state_singleton) || !actor_db || !actor_info || !delta ||
        strlen(name) >= 0x60) return -1;

    // OneShotAddOrRemovePouchContent reads the event parameters through pointers.
    const uint64_t event = (c->x[31] - 0xe0) & ~0xfULL;
    memset(A64_PTR(event), 0, 0xe0);
    memcpy(A64_PTR(event + 0x80), name, strlen(name) + 1);
    recomp_store64_sp(c, event + 0x20, event + 0x60);
    recomp_store64_sp(c, event + 0x30, event + 0x70);
    recomp_store64_sp(c, event + 0x38, event + 0x68);
    recomp_store64_sp(c, event + 0x48, event + 0x74);
    recomp_store64_sp(c, event + 0x50, event + 0x74);
    recomp_store64_sp(c, event + 0x60, event + 0x80);
    recomp_store64_sp(c, event + 0x68, 0x83f0a6d8ULL); // Script
    recomp_store32_sp(c, event + 0x70, (uint32_t)delta);
    if (!game_call(c, 0x828d4a70ULL, event, 1, (const uint64_t[]){event}, NULL)) return -1;
    return (int)(recomp_load32(c, event + 0x18) >> 30);
}

/* QueryGetPouchContentNumByActorName (f_82a3c260): f_805b63e0(pouch manager, &name, 0, 0) counts one content. */
static int oot_pouch_count(Ctx *c, const char *name) {
    const uint64_t manager = recomp_load64(c, 0x84e98e28ULL);
    if (!manager || !game_has(0x805b63e0ULL) || strlen(name) >= 0x60) return -1;
    const uint64_t frame = (c->x[31] - 0x80) & ~0xfULL;
    memcpy(A64_PTR(frame + 0x10), name, strlen(name) + 1);
    recomp_store64_sp(c, frame, frame + 0x10);
    uint64_t n;
    if (!game_call(c, 0x805b63e0ULL, frame, 4, (const uint64_t[]){manager, frame, 0, 0}, &n)) return -1;
    return (int)(int32_t)n;
}

static void oot_edit_pouch(Ctx *c, int action) {
    static const char *const items[] = {
        "Item_DekuSeed", "Item_DekuNut", "Item_DekuStick", "Item_Bomb", "Item_Arrow_Normal"
    };
    const unsigned item = (unsigned)(action - 200) / 32;
    const int delta = (action - 200) % 32 - 10;
    if (item >= sizeof items / sizeof items[0] || !delta || delta < -10 || delta > 10) return;
    const int result = oot_pouch_change(c, items[item], delta);
    if (result >= 0) hos_log("cheats: pouch %s %+d (event status %d)", items[item], delta, result);
}

/* The Inventory page (game/oot_cheat_items.h): counts go to the menu twice a second, edits come back as actions. */
#define OOT_ITEM_NAME(label, name, kind) name,
#define OOT_ITEM_KIND(label, name, kind) kind,
#define OOT_NO_SECTION(name)
static const char *const oot_item_names[] = {OOT_CHEAT_ITEMS(OOT_NO_SECTION, OOT_ITEM_NAME)};
static const unsigned char oot_item_kinds[] = {OOT_CHEAT_ITEMS(OOT_NO_SECTION, OOT_ITEM_KIND)};
#define OOT_ITEM_COUNT (sizeof oot_item_names / sizeof oot_item_names[0])

static int oot_item_count(Ctx *c, const char *name);
static const char *oot_gamedata_queue_bool(Ctx *c, uint32_t hash, int value);
static int oot_event_playing(Ctx *c);

/* The first adult night's time lock (Longshot #1064): f_8347c020 holds the clock at 20:00 while the saved Bool
   HrlField_isFirstNightTime_Adult (0x8592aa82) is 1 and quest 0x4dbca27f is not Ready. The flag belongs to Hyrule
   Field's first night only. A save in any other map that still holds it cannot leave night: clear it there with the
   game's own Bool, once, and say so, never while an event (the escape's cutscene) is running. HOS_OOT_FIRST_NIGHT_FIX=0
   turns the repair off for a run. Returns 1 when it cleared the flag. */
static int oot_first_night_fix(Ctx *c, const char *map) {
    static int on = -1;
    if (on < 0) on = env_on_or("HOS_OOT_FIRST_NIGHT_FIX", 1);
    if (!on || !map[0] || !strncmp(map, "HyruleField", 11) || oot_event_playing(c)) return 0;
    const uint64_t mgr = game_flags_mgr();
    int set = 0;
    if (!game_flag_bool(mgr, 0x8592aa82u, &set) || !set) return 0;
    if (!game_flag_set_bool(mgr, 0x8592aa82u, 0)) return 0;
    hos_log("oot: cleared the first adult night's time lock (the save was in %s, outside Hyrule Field)", map);
    return 1;
}

static void oot_inventory_refresh(Ctx *c, int64_t now) {
    void rx_option_set(int id, int value);
    static int64_t next;
    static int probed;
    if (now < next) return;
    next = now + 500000000LL;
    int loaded = 0;   /* (a save's counts read: the first item's is not -1) */
    for (unsigned i = 0; i < OOT_ITEM_COUNT; i++) {
        const int count = oot_item_count(c, oot_item_names[i]);
        if (!i) loaded = count >= 0;
        rx_option_set(OOT_CHEAT_ITEM_BASE + (int)i, count);
    }
    {   /* Link's age for the Debug tab's Switch age row: 0 child, 1 adult, -1 no save loaded (Sequence_IsChildhood) */
        int child = 1;
        rx_option_set(OOT_CHEAT_AGE_ID, loaded && game_flag_bool(game_flags_mgr(), 0xcea220d3u, &child) ? (child ? 0 : 1) : -1);
    }
    static char logged[100];
    if (!loaded && logged[0]) {   /* back to the title or the file select: the summary's time there is not the last map's */
        logged[0] = 0;
        AcquireSRWLockExclusive(&map_lock);
        map_now[0] = 0;
        ReleaseSRWLockExclusive(&map_lock);
        perf_pose_clear();   /* (nor is Link's last place in the field: the title's samples have no pose) */
    }
    if (loaded) {   /* the map Link is in and his age, logged when either changes: where a log's problem happened, the "Where in
                       the game" of the Insert menu's Bug report (gpu/vk/vk_bug_report.cpp reads the last of these lines) */
        char map[80], line[100];
        int child = 1;
        if (game_flag_text(game_flags_mgr(), 0x1d6189dau, map, sizeof map) && map[0]) {   /* Sequence_CurrentBanc */
            snprintf(line, sizeof line, "%s (%s)", map,
                     game_flag_bool(game_flags_mgr(), 0xcea220d3u, &child) ? (child ? "child" : "adult") : "age not known");
            if (strcmp(line, logged)) {
                hos_log("map: %s", line), snprintf(logged, sizeof logged, "%s", line);
                AcquireSRWLockExclusive(&map_lock);   /* (the performance summary's map: hos_perf_info) */
                snprintf(map_now, sizeof map_now, "%s", line);
                ReleaseSRWLockExclusive(&map_lock);
                perf_pose_clear();   /* (the pose is of the map before: the next update of Link's gives this one's) */
            }
            oot_first_night_fix(c, map);
        }
    }
    if (!probed && oot_pouch_count(c, "Item_DekuSeed") >= 0) {
        probed = 1;   /* (each editor value, plus the alternative pouch spellings) */
        static const char *const probes[] = {OOT_CHEAT_ITEM_PROBES};
        const unsigned total = (unsigned)(OOT_ITEM_COUNT + sizeof probes / sizeof probes[0]);
        char line[512];
        size_t used = 0;
        for (unsigned i = 0; i < total; i++) {   /* (several short lines: the log cuts long ones) */
            const char *name = i < OOT_ITEM_COUNT ? oot_item_names[i] : probes[i - OOT_ITEM_COUNT];
            const int count = i < OOT_ITEM_COUNT ? oot_item_count(c, name) : oot_pouch_count(c, name);
            used += (size_t)snprintf(line + used, sizeof line - used, " %s=%d", name, count);
            if (used > 300 || i + 1 == total) hos_log("cheats: inventory probe:%s", line), used = 0;
        }
    }
}

/* Learned songs use saved IsGet.<actor> Bool flags rather than pouch quantities.
   These are their MurmurHash3 x86-32 hashes with seed 0. */
static uint32_t oot_song_flag(const char *name) {
    static const struct { const char *name; uint32_t hash; } songs[] = {
        {"Item_Melody_ZeldaLullaby", 0x01c20002u},
        {"Item_Melody_EponaSong", 0xb492ac33u},
        {"Item_Melody_SariaSong", 0xd3f8bfccu},
        {"Item_Melody_SunSong", 0x165dd1adu},
        {"Item_Melody_TimeSong", 0x03b3450eu},
        {"Item_Melody_StormSong", 0x2b39dd4fu},
        {"Item_Melody_ForestMinuet", 0xdc0a0b54u},
        {"Item_Melody_FireBolero", 0x49041ab9u},
        {"Item_Melody_WaterSerenade", 0x3fa63986u},
        {"Item_Melody_ShadowNocturne", 0xc2fb95f4u},
        {"Item_Melody_SpiritRequiem", 0x802a36a8u},
        {"Item_Melody_LightPrelude", 0xd6320ba4u},
    };
    for (unsigned i = 0; i < sizeof songs / sizeof songs[0]; i++)
        if (!strcmp(name, songs[i].name)) return songs[i].hash;
    return 0;
}

/* Masks share one slot (the Happy Mask Shop lends one at a time). The game takes "Item_Mask_X" when adding one and
   may keep it as "Armor_Child_Mask_X", so both spellings count and a new mask first returns the one held. */
static int oot_mask_alias(const char *name, char *alias, size_t size) {
    if (!strncmp(name, "Armor_Child_Mask_", 17)) return snprintf(alias, size, "Item_Mask_%s", name + 17), 1;
    return 0;
}

static int oot_item_count(Ctx *c, const char *name) {
    const uint32_t song = oot_song_flag(name);
    if (song) {
        int learned = 0;
        return game_flag_bool(game_flags_mgr(), song, &learned) ? learned : -1;
    }
    char alias[64];
    const int n = oot_pouch_count(c, name);
    if (!oot_mask_alias(name, alias, sizeof alias)) return n;
    const int m = oot_pouch_count(c, alias);
    return n > m ? n : m;
}

static void oot_inventory_edit(Ctx *c, int action) {
    const unsigned item = (unsigned)(action - OOT_CHEAT_ITEM_ACTION) / 256, op = (unsigned)(action - OOT_CHEAT_ITEM_ACTION) % 256;
    if (item >= OOT_ITEM_COUNT) return;
    const char *name = oot_item_names[item];
    const int have = oot_item_count(c, name);
    const uint32_t song = oot_song_flag(name);
    if (song) {
        if (op != 200 || have < 0) return;   /* songs have no editable quantity */
        const char *missing = oot_gamedata_queue_bool(c, song, !have);
        if (missing) {
            hos_log("cheats: song %s: %s not found", name, missing);
            return;
        }
        hos_log("cheats: song %s: %s queued", name, have ? "relock" : "unlock");
        return;
    }
    char alias[64];
    const int is_mask = oot_mask_alias(name, alias, sizeof alias);
    int delta = 0;
    if (op == 200) delta = have > 0 ? -have : 1;   /* unlock / relock */
    else if (op < 128) delta = (int)op - 64;
    if (!delta || have < 0) return;
    if (is_mask && delta > 0) {   /* return whichever mask is in the slot first */
        for (unsigned i = 0; i < OOT_ITEM_COUNT; i++) {
            char other_alias[64];
            if (i == item || !oot_mask_alias(oot_item_names[i], other_alias, sizeof other_alias)) continue;
            const char *spellings[2] = {oot_item_names[i], other_alias};
            for (unsigned k = 0; k < 2; k++) {
                const int held = oot_pouch_count(c, spellings[k]);
                if (held > 0) hos_log("cheats: inventory returned %s (status %d)", spellings[k], oot_pouch_change(c, spellings[k], -held));
            }
        }
        delta = 1;
    }
    int result = -1;
    if (is_mask && delta < 0) {   /* remove under whichever spelling holds it */
        const char *spellings[2] = {name, alias};
        for (unsigned k = 0; k < 2; k++) {
            const int held = oot_pouch_count(c, spellings[k]);
            if (held > 0) result = oot_pouch_change(c, spellings[k], -held);
        }
    } else {
        result = oot_pouch_change(c, is_mask ? alias : name, delta);   /* (masks are added as Item_Mask_X, as the shop does) */
    }
    hos_log("cheats: inventory %s %+d (had %d, now %d, event status %d)", name, delta, have, oot_item_count(c, name), result);
}

static void oot_set_time(Ctx *c, unsigned hour) {
    // ExecuteWmSetTime passes seconds since midnight to the world manager.
    const uint64_t singleton = recomp_load64(c, 0x84c9e898ULL);
    const uint64_t world = singleton ? recomp_load64(c, singleton) : 0;
    if (!game_has(0x834722d0ULL) || !world || !recomp_load64(c, world + 0x510)) {
        hos_log("cheats: world clock unavailable");
        return;
    }
    game_call(c, 0x834722d0ULL, 0, 2, (const uint64_t[]){world, hour * 3600ULL}, NULL);
    hos_log("cheats: requested %02u:00", hour);
}

/* Cross-map teleport (build e0f3ccfa). Map changes in this game only run through an event: an entrance actor
   requests the Dm_SY_BancChangeGo event with NextBanc / NextStartPos / BootEventName arguments, the event calls
   GameSystemActor's ReserveBancChange, and the sequence's ChangeBancOnEvent loads the map when the event ends.
   This builds the same event request (see work/oot-teleport-research.md). The menu labels in gpu/vk/vk_menu.cpp
   (teleport_names) follow this table's order. */
typedef struct {
    const char *child_banc, *child_start, *child_boot;
    const char *adult_banc, *adult_start, *adult_boot;
} OotDestination;
static const OotDestination oot_destinations[] = {   /* (generated: work/oot-teleport-tools/write_destinations.py) */
    {"KokiriForest_Child", "LostWoodBridge_FrontOfHyruleField", "Dm_SY_BancChangeArrive",
     "KokiriForest_Adult", "LostWoodBridge_FrontOfHyruleField", "Dm_SY_BancChangeArrive"},   /* Kokiri Forest */
    {"HyruleField_Child", "HyruleField_FrontOfLostWoodBridge", "Dm_SY_BancChangeArrive",
     "HyruleField_Adult", "HyruleField_FrontOfLostWoodBridge", "Dm_SY_BancChangeArrive"},   /* Hyrule Field */
    {"KakarikoVillage_Child", "KakarikoVillage_Entrance", "Dm_SY_BancChangeArrive_KakarikoVillage_01",
     "KakarikoVillage_Adult", "KakarikoVillage_Entrance", "Dm_SY_BancChangeArrive_KakarikoVillage_01"},   /* Kakariko Village */
    {"Market_Child", "Market_Entrance", "Dm_SY_BancChangeArrive_Market_02",
     "Market_Adult", "Market_Entrance", "Dm_SY_BancChangeArrive_Market_02"},   /* Market */
    {"HyruleCastle", "HyruleCastle_Entrance", "Dm_SY_BancChangeArrive_HyruleCastle_01",
     "GanonCastle", "GanonCastleEntrance", "Dm_SY_BancChangeArrive_GanonCastle_01"},   /* Hyrule Castle */
    {"LostWood_Child", "LostWood_FrontOfSacredForestMeadow", "Dm_SY_BancChangeArrive_LostWood",
     "LostWood_Adult", "LostWood_FrontOfSacredForestMeadow", "Dm_SY_BancChangeArrive_LostWood"},   /* Lost Woods */
    {"SacredForestMeadow_Child", "OutsideOfForestTemple", "Dm_SY_BancChangeArrive_SacredForestMeadow_01",
     "SacredForestMeadow_Adult", "OutsideOfForestTemple", "Dm_SY_BancChangeArrive_SacredForestMeadow_01"},   /* Sacred Forest Meadow */
    {"HyruleField_Child", "LakeHylia_FromGerudoValley", "Dm_SY_BancChangeArrive_LakeHylia_02",
     "HyruleField_Adult", "LakeHylia_FromGerudoValley", "Dm_SY_BancChangeArrive_LakeHylia_02"},   /* Lake Hylia */
    {"Graveyard_Child", "GraveYard_KakarikoSide", "Dm_SY_BancChangeArrive_Graveyard_01",
     "Graveyard_Adult", "GraveYard_KakarikoSide", "Dm_SY_BancChangeArrive_Graveyard_01"},   /* Graveyard */
    {"DeathMountainTrail_Child", "MountainTrail_KakarikoSide", "Dm_SY_BancChangeArrive_DeathMountainTrail_01",
     "DeathMountainTrail_Adult", "MountainTrail_KakarikoSide", "Dm_SY_BancChangeArrive_DeathMountainTrail_01"},   /* Death Mountain Trail */
    {"DeathMountainCrater_Child", "Crater_GoronCitySide", "DeathMountainCrater_Arrive",
     "DeathMountainCrater_Adult", "Crater_GoronCitySide", "DeathMountainCrater_Arrive"},   /* Death Mountain Crater */
    {"GoronCity_Child", "GoronCity_MountainTrailSide", "Dm_SY_BancChangeArrive_GoronCity_01",
     "GoronCity_Adult", "GoronCity_MountainTrailSide", "Dm_SY_BancChangeArrive_GoronCity_01"},   /* Goron City */
    {"ZoraRiver_Child", "ZoraRiver_Entrance", "Dm_SY_BancChangeArrive",
     "ZoraRiver_Adult", "ZoraRiver_Entrance", "Dm_SY_BancChangeArrive"},   /* Zora's River */
    {"ZoraDomain_Child", "ZoraDomain_FromZoraRiver", "Dm_SY_BancChangeArrive",
     "ZoraDomain_Adult", "ZoraDomain_FromZoraRiver", "Dm_SY_BancChangeArrive"},   /* Zora's Domain */
    {"ZoraFountain_Child", "ZoraFountain_FromZoraDomain", "Dm_SY_BancChangeArrive",
     "ZoraFountain_Adult", "ZoraFountain_FromZoraDomain", "Dm_SY_BancChangeArrive"},   /* Zora's Fountain */
    {"GerudoValley_Child", "GerudoValley_Entrance", "FromHyruleField_ArriveGerudoValley",
     "GerudoValley_Adult", "GerudoValley_Entrance", "FromHyruleField_ArriveGerudoValley"},   /* Gerudo Valley */
    {NULL, NULL, NULL,
     "GerudoFortress_Adult", "GerudoFortress_FromHountedWasteland", "Dm_SY_BancChangeArrive_GerudoFortress_02"},   /* Gerudo Fortress */
    {"HountedWasteland_Child", "HountedWasteland_FromGerudoFortress", "FromGerudoFortress_ArriveWateland",
     "HountedWasteland_Adult", "HountedWasteland_FromGerudoFortress", "FromGerudoFortress_ArriveWateland"},   /* Haunted Wasteland */
    {"DesertColossus_Child", "DesertColossus_FromHountedWasteland", "Dm_SY_BancChangeArrive_DesertColossus_01",
     "DesertColossus_Adult", "DesertColossus_FromHountedWasteland", "Dm_SY_BancChangeArrive_DesertColossus_01"},   /* Desert Colossus */
    {"InsideTheDekuTree", "Entrance", "Dm_SY_BancChangeArrive_InsideTheDekuTree_01",
     "InsideTheDekuTree", "Entrance", "Dm_SY_BancChangeArrive_InsideTheDekuTree_01"},   /* Inside the Deku Tree */
    {"DodongoCavern", "DodongoCavern_MountainTrailSide", "Dm_SY_BancChangeArrive",
     "DodongoCavern", "DodongoCavern_MountainTrailSide", "Dm_SY_BancChangeArrive"},   /* Dodongo's Cavern */
    {"InsideJabuJabuBelly", "InsideJabuJabuBelly_FromZoraFountain", "Dm_SY_BancChangeArrive",
     "InsideJabuJabuBelly", "InsideJabuJabuBelly_FromZoraFountain", "Dm_SY_BancChangeArrive"},   /* Jabu-Jabu's Belly */
    {"ForestTemple", "ForestTemple", "Dm_SY_BancChangeArrive",
     "ForestTemple", "ForestTemple", "Dm_SY_BancChangeArrive"},   /* Forest Temple */
    {"FireTemple", "FireTemple", "Dm_SY_BancChangeArrive",
     "FireTemple", "FireTemple", "Dm_SY_BancChangeArrive"},   /* Fire Temple */
    {"WaterTemple", "WaterTemple_FromLakeHylia", "SageOfWater_Arrival_WaterTemple",
     "WaterTemple", "WaterTemple_FromLakeHylia", "SageOfWater_Arrival_WaterTemple"},   /* Water Temple */
    {"ShadowTemple", "Entrance", "ShadowTemple_Arrive",
     "ShadowTemple", "Entrance", "ShadowTemple_Arrive"},   /* Shadow Temple */
    {"SpiritTemple", "SpiritTemple_FromRightHand", "Dm_SY_BancChangeArrive",
     "SpiritTemple", "SpiritTemple_FromRightHand", "Dm_SY_BancChangeArrive"},   /* Spirit Temple */
    {"BottomOfTheWell", "Well_KakarikoSide", "Dm_SY_BancChangeArrive",
     "BottomOfTheWell", "Well_KakarikoSide", "Dm_SY_BancChangeArrive"},   /* Bottom of the Well */
    {"IceCavern", "IceCavern_FromZoraFountain", "SageOfWater_Arrival_IceCavern",
     "IceCavern", "IceCavern_FromZoraFountain", "SageOfWater_Arrival_IceCavern"},   /* Ice Cavern */
    {"GerudoTrainingGround", "GerudoTrainingGround_FromGerudoFortress", "FromGerudoFortress_ArriveTrainingGround",
     "GerudoTrainingGround", "GerudoTrainingGround_FromGerudoFortress", "FromGerudoFortress_ArriveTrainingGround"},   /* Gerudo Training Ground */
    {"InsideGanonCastle", "Entrance", "Dm_SY_BancChangeArrive_InsideGanonCastle_01",
     "InsideGanonCastle", "Entrance", "Dm_SY_BancChangeArrive_InsideGanonCastle_01"},   /* Ganon's Castle */
};
#define OOT_AGE_ACTION 999   /* (the Cheats page's Age row) */
#define OOT_TELEPORT_ACTION 1000   /* (pouch edits use 200 to 348) */
#define OOT_EVENTMGR_SLOT 0x84c9e090ULL   /* EventMgr, as SeqOneShotRequestEvent loads it */
#define OOT_EVENT_ARG_SIZE 0x1b20ULL      /* EventRequestArg: SeqOneShotRequestEvent reserves this much stack */

static uint64_t oot_singleton(Ctx *c, uint64_t slot) {
    const uint64_t holder = recomp_load64(c, slot);
    return holder ? recomp_load64(c, holder) : 0;
}

/* The game's Bool setter queues the live value and the copy written by the next save. */
static const char *oot_gamedata_queue_bool(Ctx *c, uint32_t hash, int value) {
    if (!game_has(0x806a2b20ULL)) return "the game's Bool setter";
    const uint64_t mgr = game_flags_mgr();
    if (!mgr) return "the game data manager";
    if (!game_call(c, 0x806a2b20ULL, 0, 3, (const uint64_t[]){mgr, value != 0, hash}, NULL)) return "a guest thread to call it on";
    return NULL;
}

/* One argument of an EventRequestArg: the array of 200-byte entries at arg+8, capacity arg+0, count arg+16.
   Entry: +0 type (1 string pointer, 4 bool), +8 name buffer, +16 its capacity, +152 the value. As
   QueryBancChangeEventRequestArg (f_829df380) fills them. A string value is a pointer the request does not copy. */
static int oot_event_arg(Ctx *c, uint64_t arg, const char *name, uint32_t type, uint64_t value) {
    const uint32_t capacity = recomp_load32(c, arg), count = recomp_load32(c, arg + 16);
    if (count >= capacity) return 0;
    const uint64_t entry = recomp_load64(c, arg + 8) + (uint64_t)count * 200;
    const uint64_t name_buf = recomp_load64(c, entry + 8);
    const uint32_t name_cap = recomp_load32(c, entry + 16);
    const size_t length = strlen(name);
    void *to = name_buf && length + 1 <= name_cap ? hos_guest_range(name_buf, length + 1) : NULL;   /* (longshot) */
    if (!to) return 0;
    memcpy(to, name, length + 1);
    if (type == 4) recomp_store8(c, entry + 152, value & 1);
    else recomp_store64(c, entry + 152, value);
    recomp_store32(c, entry, type);
    recomp_store32(c, arg + 16, count + 1);
    return 1;
}

/* (the Main sequence's "Event" test: an event is playing) */
static int oot_event_playing(Ctx *c) {
    const uint64_t events = oot_singleton(c, OOT_EVENTMGR_SLOT);
    return events && recomp_load64(c, events + 104);
}

static struct {
    int64_t requested_at, check_at;
    unsigned destination;
    const char *why, *banc;   /* (the last call's: why it was not sent, the map it was sent to; the control bridge's reply) */
    uint64_t result;          /* (and the event manager's answer to the request: 0 seen when the map did not change) */
    int resent;               /* (the follow-up sent this request again: not a third time) */
    const char *pending;      /* (the map of the last request sent, which the follow-up checks: a later call that is
                                 refused clears banc, the bridge's reply, and must not cancel this one's retry) */
} teleport;

static int oot_teleport(Ctx *c, uint64_t actor, unsigned index) {
    teleport.why = "no such destination", teleport.banc = NULL;
    if (index >= sizeof oot_destinations / sizeof oot_destinations[0]) return 0;
    const int64_t now = hos_now_ns();
    if (teleport.requested_at && now - teleport.requested_at < 8000000000LL) {
        hos_log("cheats: teleport ignored, a request is still in progress");
        teleport.why = "another teleport is still in progress (one every 8 s)";
        return 0;
    }
    const uint64_t events = oot_singleton(c, OOT_EVENTMGR_SLOT);
    /* (the game's functions it calls: the request's arg made, its starter actor linked, the request, the links released) */
    enum { MAKE_ARG = 0x80d99f10, REQUEST = 0x80d9a820, SET_LINK = 0x802dc440, RELEASE_LINK = 0x8022b6f0 };
    if (!events || !game_has(MAKE_ARG) || !game_has(REQUEST) || !game_has(SET_LINK) || !game_has(RELEASE_LINK)) {
        hos_log("cheats: teleport unavailable (event manager %#llx)", (unsigned long long)events);
        teleport.why = "the game's event manager is not there yet";
        return 0;
    }
    if (recomp_load64(c, events + 104)) {   /* (the Main sequence's "Event" test: an event is playing) */
        hos_log("cheats: teleport waits until the current event ends");
        teleport.why = "an event is playing (a cutscene, a map change's)";
        return 0;
    }
    int loading = 0, child = 1;
    const uint64_t mgr = game_flags_mgr();
    if (game_flag_bool(mgr, 0xef2b60c2u, &loading) && loading) {   /* Sequence_IsLoadingBanc */
        hos_log("cheats: teleport ignored while a map loads");
        teleport.why = "a map is loading";
        return 0;
    }
    if (!game_flag_bool(mgr, 0xcea220d3u, &child)) hos_log("cheats: Sequence_IsChildhood not found, using child maps");
    const OotDestination *d = &oot_destinations[index];
    const char *banc = child ? d->child_banc : d->adult_banc;
    const char *start = child ? d->child_start : d->adult_start;
    const char *boot = child ? d->child_boot : d->adult_boot;
    if (!banc) {
        hos_log("cheats: teleport %u has no %s map", index, child ? "child" : "adult");
        teleport.why = child ? "this destination has no map for the child" : "this destination has no map for the adult";
        return 0;
    }

    /* The event reads its string arguments after this request returns, so they live in memory that stays. */
    static uint64_t strings;
    if (!strings) strings = hos_sys_alloc(512, 16);
    if (!strings) return teleport.why = "out of memory", 0;
    memset(A64_PTR(strings), 0, 512);
    memcpy(A64_PTR(strings), banc, strlen(banc) + 1);
    memcpy(A64_PTR(strings + 128), start, strlen(start) + 1);
    memcpy(A64_PTR(strings + 256), boot, strlen(boot) + 1);

    const char *event_env = getenv("HOS_OOT_TELEPORT_EVENT");
    const char *event = event_env && *event_env ? event_env : "Dm_SY_BancChangeGo";
    const char *starter_env = getenv("HOS_OOT_TELEPORT_STARTER");
    const int with_starter = !(starter_env && !strcmp(starter_env, "0"));

    const uint64_t arg = (c->x[31] - OOT_EVENT_ARG_SIZE - 0x100) & ~0xfULL;
    int ok = game_call(c, MAKE_ARG, arg, 1, (const uint64_t[]){arg}, NULL);
    {   /* the event's name: arg+6800 points at its buffer, arg+6808 the capacity */
        const uint64_t buf = recomp_load64(c, arg + 6800);
        const uint32_t cap = recomp_load32(c, arg + 6808);
        void *to = buf && strlen(event) + 1 <= cap ? hos_guest_range(buf, strlen(event) + 1) : NULL;   /* (longshot) */
        if (!to) ok = 0;
        else if (ok) memcpy(to, event, strlen(event) + 1);
    }
    ok = ok && oot_event_arg(c, arg, "NextBanc", 1, strings);
    ok = ok && oot_event_arg(c, arg, "NextStartPos", 1, strings + 128);
    ok = ok && oot_event_arg(c, arg, "BootEventName", 1, strings + 256);
    ok = ok && oot_event_arg(c, arg, "IsCheckAir", 4, 0);
    if (ok && with_starter)   /* the starter actor (arg+0x1950), as OneShotEventMemberRequestEventByEventName sets it */
        game_call(c, SET_LINK, arg, 3, (const uint64_t[]){arg + 0x1950, actor, 0}, NULL);
    uint64_t result = 0;
    if (ok) game_call(c, REQUEST, arg, 2, (const uint64_t[]){events, arg}, &result);
    /* SeqOneShotRequestEvent's cleanup of the arg's two actor links */
    for (unsigned i = 0; i < 2; i++) {
        const uint64_t link = arg + (i ? 0x1950 : 0x1968);
        recomp_store64(c, link, 0x84994b20ULL);
        game_call(c, RELEASE_LINK, arg, 1, (const uint64_t[]){link}, NULL);
    }
    /* A map-hiding area can leave this saved flag set when a debug teleport skips its exit.
       Only clear it once the event manager takes the request; the destination may hide its own minimap.
       Use the game's Bool setter (mgr, value, hash): it queues the change for the live value AND the save's copy.
       Writing only the live byte restores the HUD until the next load, but saves keep the old hidden value. */
    int hidden = 0;
    if (ok && result && game_flag_bool(mgr, 0x9a4dd779u, &hidden) && hidden) {
        const char *missing = oot_gamedata_queue_bool(c, 0x9a4dd779u, 0);   /* MapIconDeactivateByAreaActor */
        if (!missing) {
            hos_log("cheats: teleport: the area left had hidden the minimap (MapIconDeactivateByAreaActor): restore queued");
        } else {   /* (another game build: say so, a report's log then shows why the minimap stayed away) */
            hos_log("cheats: teleport: the area left had hidden the minimap (MapIconDeactivateByAreaActor): %s not found, left hidden",
                    missing);
        }
    }
    teleport.requested_at = now;
    teleport.check_at = now + 1500000000LL;
    teleport.resent = 0;   /* (a new request: the follow-up may send it once more) */
    teleport.destination = index;
    teleport.pending = ok ? banc : NULL;
    hos_log("cheats: teleport %s to %s at %s (boot %s, event %s, starter %d): request %s, result %#llx",
            child ? "child" : "adult", banc, start, boot, event, with_starter, ok ? "sent" : "not built",
            (unsigned long long)result);
    teleport.result = result;
    if (ok) teleport.banc = banc;
    else teleport.why = "the game's event request could not be built";
    return ok;   /* (sent; the request's own result is only logged) */
}

/* Age toggle: flips Sequence_IsChildhood (what the Temple of Time event sets) and reloads the current map, or the
   Market when this map is not in oot_destinations, through the teleport so the other age's map and Link load. 1: the map
   change was sent (teleport.banc its map); 0: not, teleport.why says why (the control bridge's reply) */
static int oot_toggle_age(Ctx *c, uint64_t actor) {
    int child = 1;
    if (!game_flag_bool(game_flags_mgr(), 0xcea220d3u, &child)) {
        hos_log("cheats: age toggle: Sequence_IsChildhood not found");
        teleport.why = "Link's age is not in the game's data (Sequence_IsChildhood not found)", teleport.banc = NULL;
        return 0;
    }
    char current[80];
    game_flag_text(game_flags_mgr(), 0x1d6189dau, current, sizeof current);   /* Sequence_CurrentBanc */
    unsigned target = 3;   /* Market (the Temple of Time is in it) */
    for (unsigned i = 0; i < sizeof oot_destinations / sizeof oot_destinations[0]; i++) {
        const OotDestination *d = &oot_destinations[i];
        const char *other = child ? d->adult_banc : d->child_banc;
        if (other && ((d->child_banc && !strcmp(d->child_banc, current)) || (d->adult_banc && !strcmp(d->adult_banc, current)))) {
            target = i;
            break;
        }
    }
    if (!game_flag_set_bool(game_flags_mgr(), 0xcea220d3u, !child)) {
        hos_log("cheats: age toggle: could not write Sequence_IsChildhood");
        teleport.why = "Link's age could not be written (Sequence_IsChildhood)", teleport.banc = NULL;
        return 0;
    }
    hos_log("cheats: age toggle: %s -> %s, reloading via destination %u (current map \"%s\")",
            child ? "child" : "adult", child ? "adult" : "child", target, current);
    if (!oot_teleport(c, actor, target)) {
        game_flag_set_bool(game_flags_mgr(), 0xcea220d3u, child);
        hos_log("cheats: age toggle undone, the map change was not requested");
        return 0;
    }
    return 1;
}

static void oot_teleport_followup(Ctx *c, uint64_t actor) {
    if (!teleport.check_at || hos_now_ns() < teleport.check_at) return;
    teleport.check_at = 0;
    char next[80] = "", current[80] = "";
    game_flag_text(game_flags_mgr(), 0xee27bf71u, next, sizeof next);     /* Sequence_ReservedNextBanc */
    game_flag_text(game_flags_mgr(), 0x1d6189dau, current, sizeof current);   /* Sequence_CurrentBanc */
    const uint64_t events = oot_singleton(c, OOT_EVENTMGR_SLOT);
    const int playing = events && recomp_load64(c, events + 104) ? 1 : 0;
    int loading = 0;
    game_flag_bool(game_flags_mgr(), 0xef2b60c2u, &loading);   /* Sequence_IsLoadingBanc */
    hos_log("cheats: teleport check: reserved \"%s\", current \"%s\", event playing %d", next, current, playing);
    /* (dropped by the game, as a request a few frames after a save has loaded is: sent once more, the 8 s guard aside, so
       the control bridge's wait for the map sees it arrive. oot_teleport_check.h) */
    if (oot_teleport_dropped(next, current, teleport.pending, playing, loading, teleport.result, teleport.resent)) {
        hos_log("cheats: teleport to %s dropped by the game (answered 0, nothing reserved, no event): sending it again",
                teleport.pending);
        teleport.requested_at = 0;
        const int sent = oot_teleport(c, actor, teleport.destination);
        teleport.resent = 1;   /* (oot_teleport set it for a new request: this one is the same) */
        if (!sent) hos_log("cheats: teleport to %s: not sent again: %s", teleport.pending, teleport.why);
    }
}

/* (longshot) the control bridge (host/control.c): the map the game is in (Sequence_CurrentBanc), read on the game's thread
   with the player's update every 250 ms and kept for the bridge's thread; the last one known (it stays through loads and
   menus, where Link is not updated). And the debug teleport by destination, as the Cheats page asks for it: taken at the
   next update of the player, in play, and what came of it (sent, or refused and why) kept for the bridge's reply */
static SRWLOCK oot_map_lock = SRWLOCK_INIT;
static char oot_map[80];
static int oot_flags = -1;   /* (1 an event playing, 2 a map loading, 4 the child; -1 not read yet) */
/* (with the map, for the Discord status: Link's life and its most in quarter hearts, his rupees, his Gold Skulltula
   tokens, his pieces of heart; -1 not known) */
static int oot_stats[5] = {-1, -1, -1, -1, -1};
static void oot_map_refresh(Ctx *c, int64_t now) {
    static int64_t next;
    if (now < next) return;
    next = now + 250000000LL;
    char map[80];
    const uint64_t mgr = game_flags_mgr();
    if (!game_flag_text(mgr, 0x1d6189dau, map, sizeof map) || !map[0]) return;   /* Sequence_CurrentBanc */
    /* (a flag not found reads 0, as the game's getter this called before wrote it: not loading, the adult) */
    int loading, child;
    if (!game_flag_bool(mgr, 0xef2b60c2u, &loading)) loading = 0;   /* Sequence_IsLoadingBanc */
    if (!game_flag_bool(mgr, 0xcea220d3u, &child)) child = 0;       /* Sequence_IsChildhood */
    const uint64_t events = oot_singleton(c, OOT_EVENTMGR_SLOT);
    const int flags = (events && recomp_load64(c, events + 104) ? 1 : 0) | (loading ? 2 : 0) | (child ? 4 : 0);
    int stats[5] = {-1, -1, -1, -1, -1};
    static const uint32_t hashes[3] = {0x31ab5580u, 0xfbe01da1u, 0xa77921d7u};   /* PlayerStatus.Life, .MaxLife, .CurrentRupee */
    for (int i = 0; i < 3; i++) {
        int64_t v;
        if (game_flag_number(mgr, GAME_FLAG_INT, hashes[i], &v) && v >= 0 && v < 100000) stats[i] = (int)v;
    }
    /* (the Gold Skulltula tokens, as the Cheats page counts them, -1 none: a call into the game, so every 2 s, not 250 ms; the
       Discord status changes at most every 15 s) */
    static int tokens = -1, pieces = -1;
    static unsigned refreshes;
    if (refreshes++ % 8 == 0) {
        tokens = oot_pouch_count(c, "Item_Token_Gold");
        /* (the pieces of heart he found: from those he holds, 0-3, Pouch_Heart_Piece's pouch content by its actor name,
           his most life and the boss containers he took; -1 until game/oot_heart_containers.c has their ids) */
        pieces = heart_pieces_total(stats[1], hos_oot_heart_containers_taken(), oot_pouch_count(c, "Obj_Heart_Piece"));
    }
    stats[3] = tokens, stats[4] = pieces;
    AcquireSRWLockExclusive(&oot_map_lock);
    memcpy(oot_map, map, sizeof oot_map);
    oot_flags = flags;
    memcpy(oot_stats, stats, sizeof oot_stats);
    ReleaseSRWLockExclusive(&oot_map_lock);
}
/* (with the map, game/discord_rpc.c) Link's life and its most in quarter hearts, his rupees, Gold Skulltula tokens and pieces
   of heart, each -1 when not known; 0 before any */
int hos_oot_stats(int *life, int *max_life, int *rupees, int *tokens, int *pieces) {
    AcquireSRWLockShared(&oot_map_lock);
    const int have = oot_flags >= 0;
    *life = oot_stats[0], *max_life = oot_stats[1], *rupees = oot_stats[2], *tokens = oot_stats[3], *pieces = oot_stats[4];
    ReleaseSRWLockShared(&oot_map_lock);
    return have;
}
/* (with the map) whether an event (a cutscene, a map change's) is playing, a map is loading, Link is the child: 0 before any */
int game_state(int *event, int *loading, int *child) {
    AcquireSRWLockShared(&oot_map_lock);
    const int f = oot_flags;
    ReleaseSRWLockShared(&oot_map_lock);
    if (f < 0) return 0;
    *event = f & 1, *loading = (f >> 1) & 1, *child = (f >> 2) & 1;
    return 1;
}
int game_current_map(char *out, size_t cap) {
    AcquireSRWLockShared(&oot_map_lock);
    const int have = oot_map[0] != 0;
    if (have) snprintf(out, cap, "%s", oot_map);
    ReleaseSRWLockShared(&oot_map_lock);
    return have;
}
int game_destination(int index, const char **child, const char **adult) {
    if (index < 0 || index >= (int)(sizeof oot_destinations / sizeof oot_destinations[0])) return 0;
    *child = oot_destinations[index].child_banc, *adult = oot_destinations[index].adult_banc;
    return 1;
}
/* The bridge's teleport: asked on its thread, taken at the player's next update (oot_bridge_teleport_take), and what came
   of it kept for the bridge, by ticket. A later ask replaces one not taken yet. (oot_map_lock) */
#define OOT_BRIDGE_AGE 0x10000   /* (oot_bridge_tp.asked: the age switch to the child, | 1 to the adult; not a destination) */
static struct {
    int asked;    /* the destination + 1 waiting to be taken, OOT_BRIDGE_AGE | adult the age switch; 0 none */
    int ticket;   /* the last ask's */
    int taking;   /* the ticket taken by the player's update and not done yet (it is not replaced meanwhile) */
    int done_ticket, result, accepted;   /* the last one taken: 1 sent, -1 refused, 2 (the age switch) Link was that age
                                            already; accepted: the event manager's answer */
    char why[96], map[80];
} oot_bridge_tp;
int game_teleport_ask(int index) {
    if (index < 0 || index >= (int)(sizeof oot_destinations / sizeof oot_destinations[0])) return 0;
    AcquireSRWLockExclusive(&oot_map_lock);
    const int ticket = ++oot_bridge_tp.ticket;
    __atomic_store_n(&oot_bridge_tp.asked, index + 1, __ATOMIC_RELAXED);   /* (read without the lock first: take) */
    ReleaseSRWLockExclusive(&oot_map_lock);
    return ticket;
}
/* The bridge's age switch to the adult (1) or the child (0): the Debug tab's Switch age (oot_toggle_age), asked and taken as
   a teleport is, with the same tickets, result and cancel. The age wanted goes with it and is checked when it is taken (the
   bridge's sample of Link's age can be old): already that age, nothing is done (result 2) */
int game_age_ask(int adult) {
    AcquireSRWLockExclusive(&oot_map_lock);
    const int ticket = ++oot_bridge_tp.ticket;
    __atomic_store_n(&oot_bridge_tp.asked, OOT_BRIDGE_AGE | (adult ? 1 : 0), __ATOMIC_RELAXED);
    ReleaseSRWLockExclusive(&oot_map_lock);
    return ticket;
}
/* 1 sent (map: the map it goes to; accepted: whether the game's event manager took the request), -1 refused (why: the
   reason), 2 (the age switch) Link was that age already, nothing done, 0 not taken yet (or being taken), -2 replaced by a
   later ask or cancelled */
int game_teleport_result(int ticket, char *why, size_t why_cap, char *map, size_t map_cap, int *accepted) {
    AcquireSRWLockShared(&oot_map_lock);
    int r = -2;
    if (oot_bridge_tp.done_ticket == ticket) {
        r = oot_bridge_tp.result, *accepted = oot_bridge_tp.accepted;
        snprintf(why, why_cap, "%s", oot_bridge_tp.why);
        snprintf(map, map_cap, "%s", oot_bridge_tp.map);
    } else if (oot_bridge_tp.taking == ticket || (oot_bridge_tp.ticket == ticket && oot_bridge_tp.asked)) r = 0;
    ReleaseSRWLockShared(&oot_map_lock);
    return r;
}
void game_teleport_cancel(int ticket) {   /* (the bridge gave up waiting: not taken later behind its back) */
    AcquireSRWLockExclusive(&oot_map_lock);
    if (oot_bridge_tp.ticket == ticket) __atomic_store_n(&oot_bridge_tp.asked, 0, __ATOMIC_RELAXED);
    ReleaseSRWLockExclusive(&oot_map_lock);
}
static int oot_bridge_teleport_take(int *ticket) {   /* (the player's update) the destination + 1 asked for, or 0 */
    if (!__atomic_load_n(&oot_bridge_tp.asked, __ATOMIC_RELAXED)) return 0;
    AcquireSRWLockExclusive(&oot_map_lock);
    const int asked = oot_bridge_tp.asked;
    *ticket = oot_bridge_tp.ticket;
    if (asked) oot_bridge_tp.taking = *ticket;   /* (still "not taken yet" to the bridge until done) */
    __atomic_store_n(&oot_bridge_tp.asked, 0, __ATOMIC_RELAXED);
    ReleaseSRWLockExclusive(&oot_map_lock);
    return asked;
}
static void oot_bridge_teleport_done(int ticket, int sent) {   /* sent: 1, 0 refused (teleport.why), 2 already that age */
    AcquireSRWLockExclusive(&oot_map_lock);
    if (sent == 2) {
        oot_bridge_tp.done_ticket = ticket, oot_bridge_tp.result = 2, oot_bridge_tp.taking = 0, oot_bridge_tp.accepted = 0;
        oot_bridge_tp.why[0] = oot_bridge_tp.map[0] = 0;
        ReleaseSRWLockExclusive(&oot_map_lock);
        return;
    }
    oot_bridge_tp.done_ticket = ticket, oot_bridge_tp.result = sent ? 1 : -1, oot_bridge_tp.taking = 0;
    oot_bridge_tp.accepted = sent && teleport.result != 0;
    snprintf(oot_bridge_tp.why, sizeof oot_bridge_tp.why, "%s", sent || !teleport.why ? "" : teleport.why);
    snprintf(oot_bridge_tp.map, sizeof oot_bridge_tp.map, "%s", sent && teleport.banc ? teleport.banc : "");
    ReleaseSRWLockExclusive(&oot_map_lock);
}

static uint64_t oot_gameplay_view(Ctx *c) {
    const uint64_t singleton = recomp_load64(c, 0x84ca6bc0ULL);
    const uint64_t manager = singleton ? recomp_load64(c, singleton) : 0;
    const uint64_t cameras = manager && recomp_load32(c, manager + 32) ? recomp_load64(c, manager + 40) : 0;
    const uint64_t camera = cameras ? recomp_load64(c, cameras) : 0;
    return camera ? recomp_load64(c, camera + 120) : 0;
}

static float oot_load_float(Ctx *c, uint64_t addr) {
    const uint32_t bits = recomp_load32(c, addr);
    float value;
    memcpy(&value, &bits, sizeof value);
    return value;
}

static double oot_load_double(Ctx *c, uint64_t addr) {
    const uint64_t bits = recomp_load64(c, addr);
    double value;
    memcpy(&value, &bits, sizeof value);
    return value;
}

/* The nearest float that is no further from 0 than the double (the float nearest to pi is above it, and the relay refuses
   an angle above pi) */
static float oot_float_within(double v) {
    const float f = (float)v;
    return fabs((double)f) > fabs(v) ? nextafterf(f, 0.0f) : f;
}

/* The gameplay camera's yaw and pitch in radians, from its view's eye (+96) and target (+120) as the control bridge's camera
   is (oot_state_tap.c state_camera): yaw 0 toward +z and pi/2 toward +x, in [-pi, pi]; pitch 0 level, positive looking up,
   in [-pi/2, pi/2]: the range the relay takes (one sample outside it and it refuses the whole summary). Not the fields at
   +0x1a4 and +0x18c of this view: they read 0.0 all through a game (the angles the mouse turns are on the player's own
   camera object, another one, whose address changes). 0 when the view has no direction (looking straight up or down, the
   eye on the target) or a value no place has (a stray read) */
static int oot_camera_angles(Ctx *c, uint64_t view, float *yaw, float *pitch) {
    double eye[3], target[3];
    for (int i = 0; i < 3; i++)
        eye[i] = oot_load_double(c, view + 96 + (uint64_t)i * 8), target[i] = oot_load_double(c, view + 120 + (uint64_t)i * 8);
    const double x = target[0] - eye[0], y = target[1] - eye[1], z = target[2] - eye[2];
    if (!isfinite(x) || !isfinite(y) || !isfinite(z) || !(x * x + z * z > 1e-9)) return 0;
    *yaw = oot_float_within(atan2(x, z)), *pitch = oot_float_within(atan2(y, hypot(x, z)));
    return 1;
}

/* The opt-in performance sample's latest Link position and gameplay camera angles. No game memory is read while unticked,
   and at most every PERF_POSE_EVERY_NS while ticked (the samples take it once a second: not a lock and the camera's
   pointers at every update). Camera zero's view is the gameplay one only while it is that type, as the flight and the free
   camera check: in a cutscene it can be another object, whose floats mean nothing; and while an event plays there is no
   pose at all (the flight and the field of view stop for one as well: the event's camera keeps its authored angle, and
   Link is updated all through it). A value no place or view has (a stray read) is no pose either */
static void oot_perf_pose(Ctx *c, uint64_t actor) {
    if (!InterlockedCompareExchange(&perf_pose_enabled, 0, 0)) return;
    static int64_t last;   /* (Link's update's, which holds cheats_lock) */
    const int64_t now = hos_now_ns();
    if (now - last < PERF_POSE_EVERY_NS) return;
    last = now;
    const uint64_t view = oot_gameplay_view(c);
    float pose[5];
    if (oot_event_playing(c) || !view || recomp_load64(c, view) != 0x84c18a88ULL || !oot_camera_angles(c, view, &pose[3], &pose[4])) {
        perf_pose_clear();
        return;
    }
    for (int i = 0; i < 3; i++) {
        pose[i] = oot_load_float(c, actor + 796 + (uint64_t)i * 4);
        if (!isfinite(pose[i]) || fabsf(pose[i]) > 1e6f) {
            perf_pose_clear();
            return;
        }
    }
    AcquireSRWLockExclusive(&perf_pose_lock);
    memcpy(perf_pose_now, pose, sizeof pose);
    perf_pose_valid = 1, perf_pose_ns = now;
    ReleaseSRWLockExclusive(&perf_pose_lock);
}

static _Thread_local uint64_t flight_transform_controller;
static _Thread_local uint64_t flight_pose_actor;
static _Thread_local float flight_transform_yaw;
static int64_t flight_player_seen;

/* Link's controller (vtable 0x84c54f10, the one OneShotPhysicsCctChangePosition moves) from his components, 0 when the chain
   is not there */
static uint64_t oot_player_controller(Ctx *c, uint64_t components) {
    const unsigned offsets[] = {80, 32, 16, 8};
    uint64_t controller = components;
    for (unsigned i = 0; i < sizeof offsets / sizeof offsets[0]; i++) {
        controller = recomp_load64(c, controller + offsets[i]);
        if (!controller) return 0;
    }
    return recomp_load64(c, controller) == 0x84c54f10ULL ? controller : 0;
}

/* Movement speed (the Debug tab's row, HOS_OPT_CHEAT_SPEED in percent): Link's controller and the percent for
   hos_oot_cct_speed, which scales the forward speed the game sets on it. (The controller's speed rate beside it, +476, is no
   use: the game sets it again before each step.) */
static void oot_player_speed(Ctx *c, uint64_t components) {
    int rx_option(int id, int otherwise);
    static int logged = 100;
    int percent = rx_option(HOS_OPT_CHEAT_SPEED, 100);
    if (percent < 100 || percent > 400) percent = 100;
    __atomic_store_n(&oot_speed_controller, percent > 100 ? oot_player_controller(c, components) : 0, __ATOMIC_RELAXED);
    __atomic_store_n(&oot_speed_percent, percent, __ATOMIC_RELAXED);
    if (percent != logged) hos_log("cheats: movement speed %d%%", percent);
    logged = percent;
}

/* Whether the game moved Link since the flight last placed him: he is more than 2 units (the flight moves him 4 a second)
   from where it holds him. A respawn after a fall or a warp keeps his actor, so the flight took him straight back to
   where it had him, over the same drop: he fell, respawned and fell again until the flight was switched off. It starts
   again from where the game put him instead. */
static int oot_flight_moved_away(Ctx *c, uint64_t actor, const float held[3]) {
    float apart = 0;
    for (unsigned i = 0; i < 3; i++) {
        const float d = oot_load_float(c, actor + 796 + i * 4) - held[i];
        apart += d * d;
    }
    return apart > 2.0f * 2.0f;
}

static void oot_update_player_cheats(Ctx *c, uint64_t actor, uint64_t components) {
    int rx_option(int id, int otherwise);
    static uint64_t flying_actor, marked_actor, marked_controller;
    static int64_t last_tick, last_player_tick;
    static float target[3], velocity[3], mark[3], yaw;
    oot_perf_pose(c, actor);
    int flying = rx_option(HOS_OPT_CHEAT_FLIGHT, 0) > 0;
    /* An event that takes Link (a map change he flew into, the age toggle's, a cutscene) waits for him to stand, and the
       flight holds him in the air: the screen the event had faded stayed black until the flight was switched off, and a
       second request met "waits until the current event ends". While an event plays he is let go and lands; the flight
       takes him again, from where he then stands, when it has ended. */
    if (flying && oot_event_playing(c)) flying = 0;
    if (!flying) flying_actor = 0;
    const int64_t now = hos_now_ns();
    __atomic_store_n(&flight_player_seen, now, __ATOMIC_RELAXED);
    if (marked_actor != actor || now - last_player_tick > 2000000000LL) marked_controller = 0;
    last_player_tick = now;
    int action = rx_option(HOS_OPT_CHEAT_ACTION, 0);
    if (!action) action = __atomic_exchange_n(&game_live_action, 0, __ATOMIC_RELAXED);
    int bridge_ticket = 0;   /* (the control bridge's teleport or age switch, host/control.c: what came of it is kept for its reply) */
    int bridge_adult = -1;   /* (the bridge's age switch: the age wanted, 1 the adult) */
    if (!action) {
        const int asked = oot_bridge_teleport_take(&bridge_ticket);
        if (asked & OOT_BRIDGE_AGE) action = OOT_AGE_ACTION, bridge_adult = asked & 1;
        else if (asked) action = OOT_TELEPORT_ACTION + asked - 1;
    }
    {   /* (longshot, issue 1113) an input recording's playback gives the recording's actions at their frame and takes none
           of the player's (the menu's, HOS_INPUT_LIVE's, the bridge's, which is told so): host/input_movie.c */
        int hos_input_movie_action(int live, int *dropped);
        int dropped;
        const int live = action;
        action = hos_input_movie_action(live, &dropped);
        if (dropped && bridge_ticket) {
            teleport.why = "an input recording is playing: its own teleports and age switches are taken";
            oot_bridge_teleport_done(bridge_ticket, 0);
            bridge_ticket = 0;
        }
        if (action != live) bridge_adult = -1;   /* (the recording's age switch: a switch, as it was taken) */
    }
    int taken = action;   /* (what the game took, for an input recording: not an age switch to the age Link has) */
    oot_teleport_followup(c, actor);
    oot_inventory_refresh(c, now);
    oot_player_speed(c, components);
    oot_map_refresh(c, now);
    if (action >= OOT_CHEAT_ITEM_ACTION && action < OOT_CHEAT_ITEM_ACTION + 256 * (int)OOT_ITEM_COUNT) {
        oot_inventory_edit(c, action);
    } else if (action == OOT_AGE_ACTION) {
        int child = 1;   /* (the bridge's: Link's age now, not its sample's; already the age wanted, nothing to flip) */
        if (bridge_adult >= 0 && game_flag_bool(game_flags_mgr(), 0xcea220d3u, &child) && !child == bridge_adult) {
            hos_log("cheats: age toggle: Link is the %s already", child ? "child" : "adult");
            oot_bridge_teleport_done(bridge_ticket, 2);
            taken = 0;
        } else {
            const int sent = oot_toggle_age(c, actor);
            if (bridge_ticket) oot_bridge_teleport_done(bridge_ticket, sent);
        }
    } else if (action >= OOT_TELEPORT_ACTION) {
        const int sent = oot_teleport(c, actor, (unsigned)(action - OOT_TELEPORT_ACTION));
        if (bridge_ticket) oot_bridge_teleport_done(bridge_ticket, sent);
    } else if (action >= 200) {
        oot_edit_pouch(c, action);
    } else if (action >= 100 && action < 124) {
        oot_set_time(c, action - 100);
    }
    void hos_input_movie_action_taken(int a);
    hos_input_movie_action_taken(taken);   /* (recording: the action at its frame) */
    if (!flying && action != 2 && action != 3) return;

    // Follow OneShotPhysicsCctChangePosition to Link's controller.
    const uint64_t controller = oot_player_controller(c, components);
    if (!controller || recomp_load64(c, 0x84c54f10ULL + 0xe8) != 0x806d2530ULL) return;
    if (!game_has(0x806d2530ULL)) return;

    if (action == 2) {
        for (unsigned i = 0; i < 3; i++) {
            const uint32_t bits = recomp_load32(c, actor + 796 + i * 4);
            memcpy(&mark[i], &bits, sizeof bits);
        }
        marked_actor = actor;
        marked_controller = controller;
        hos_log("cheats: marked position %.2f %.2f %.2f", mark[0], mark[1], mark[2]);
    }
    const int returning = action == 3 && marked_controller == controller;
    if (action == 3 && !returning) hos_log("cheats: no mark for this player controller");
    if (!flying && !returning) return;
    const int64_t elapsed = now - last_tick;
    if (flying_actor != actor)
        yaw = atan2f(oot_load_float(c, actor + 816), oot_load_float(c, actor + 840));
    if (returning) {
        memcpy(target, mark, sizeof target);
        memset(velocity, 0, sizeof velocity);
        flying_actor = flying ? actor : 0;
    } else if (flying_actor != actor || elapsed < 0 || elapsed > 250000000 || oot_flight_moved_away(c, actor, target)) {
        for (unsigned i = 0; i < 3; i++) target[i] = oot_load_float(c, actor + 796 + i * 4);
        memset(velocity, 0, sizeof velocity);
        flying_actor = actor;
        last_tick = now;
        return;
    } else if (elapsed == 0) {
        return;
    }
    last_tick = now;

    DWORD foreground = 0;
    GetWindowThreadProcessId(GetForegroundWindow(), &foreground);
    if (!returning) {
        const float dt = fminf((float)elapsed * 1e-9f, 0.05f);
        float desired[3] = {0};
        if (foreground == GetCurrentProcessId() && !rx_option(1, 0) && !rx_option(HOS_OPT_CHEAT_CAMERA, 0)) {
            uint64_t hos_flight_input(void);
            const uint64_t axes = hos_flight_input();
            const float right = (int16_t)axes / 32767.0f, forward = (int16_t)(axes >> 16) / 32767.0f;
            const int vertical = (GetAsyncKeyState(VK_PRIOR) < 0) - (GetAsyncKeyState(VK_NEXT) < 0);
            const uint64_t view = oot_gameplay_view(c);
            float rx = 1, rz = 0;
            if (view && recomp_load64(c, view) == 0x84c18a88ULL) {
                const float x = oot_load_float(c, view + 8), z = oot_load_float(c, view + 16);
                const float length = hypotf(x, z);
                if (length > 0.01f) rx = x / length, rz = z / length;
            }
            desired[0] = rx * right + rz * forward;
            desired[1] = vertical ? vertical : (int16_t)(axes >> 32);
            desired[2] = rz * right - rx * forward;
        }
        const float length = sqrtf(desired[0] * desired[0] + desired[1] * desired[1] + desired[2] * desired[2]);
        const float speed = 4.0f / fmaxf(1.0f, length);
        const float blend = 1.0f - expf(-12.0f * dt);
        for (unsigned i = 0; i < 3; i++) {
            velocity[i] += (desired[i] * speed - velocity[i]) * blend;
            target[i] += velocity[i] * dt;
        }
        if (hypotf(desired[0], desired[2]) > 0.08f) {
            const float delta = remainderf(atan2f(desired[0], desired[2]) - yaw, 6.28318531f);
            yaw += fmaxf(-8.0f * dt, fminf(delta, 8.0f * dt));
        }
    }

    const uint64_t arg = c->x[31] - 16;   /* (the position below the stack, x1 pointing at it, the call's stack) */
    for (unsigned i = 0; i < 3; i++) {
        uint32_t bits;
        memcpy(&bits, &target[i], sizeof bits);
        recomp_store32_sp(c, arg + i * 4, bits);
    }
    recomp_store32_sp(c, arg + 12, 0);
    flight_transform_yaw = yaw;
    flight_pose_actor = flying && !returning ? actor : 0;
    flight_transform_controller = flying && !returning ? controller : 0;
    const GameCallMore lr = {.set_lr = 1, .lr = c->x[30]};   /* (LR as the update had it, as before) */
    game_call_more(c, 0x806d2530ULL, arg, 2, (const uint64_t[]){controller, arg}, NULL, &lr);
    flight_transform_controller = 0;
    if (returning) hos_log("cheats: returned to %.2f %.2f %.2f", mark[0], mark[1], mark[2]);
}
#endif

/* (game/discord_rpc.c) when Link's update last ran (hos_now_ns), 0 never: the place in the Discord status is his map's while
   he has been seen lately */
int64_t hos_oot_player_seen_ns(void) {
#if defined(_WIN32) || defined(__linux__)
    return __atomic_load_n(&flight_player_seen, __ATOMIC_RELAXED);
#else
    return 0;
#endif
}

/* (game/oot_autoload.c) whether the game is loading a map: GameDataMgr's Sequence_IsLoadingBanc; 0 before GameDataMgr is made
   or without the flag. Any thread: one byte of the game's, read */
int hos_oot_loading(void) {
    int loading = 0;
    return game_flag_bool(game_flags_mgr(), 0xef2b60c2u, &loading) && loading;   /* Sequence_IsLoadingBanc */
}

int hos_oot_player_seen(void) {
#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
    const int64_t seen = __atomic_load_n(&flight_player_seen, __ATOMIC_RELAXED);
    return seen && hos_now_ns() - seen < 250000000;
#else
    return 0;
#endif
}

int game_flight_shortcut_ready(void) {
    return hos_oot_player_seen();
}

#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
static void oot_write_flight_rotation(Ctx *c, uint64_t matrix) {
    const float s = sinf(flight_transform_yaw), co = cosf(flight_transform_yaw);
    const float rotation[3][3] = {{co, 0, s}, {0, 1, 0}, {-s, 0, co}};
    for (unsigned row = 0; row < 3; row++) {
        for (unsigned col = 0; col < 3; col++) {
            uint32_t bits;
            memcpy(&bits, &rotation[row][col], sizeof bits);
            recomp_store32(c, matrix + row * 16 + col * 4, bits);
        }
    }
}
#endif

void hos_oot_flight_rotation(Ctx *c, uint64_t controller, uint64_t matrix) {
#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
    if (flight_transform_controller == controller) oot_write_flight_rotation(c, matrix);
#else
    (void)c;
    (void)controller;
    (void)matrix;
#endif
}

void hos_oot_flight_pose(Ctx *c, uint64_t actor, uint64_t matrix) {
#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
    if (flight_pose_actor == actor) {
        flight_pose_actor = 0;
        oot_write_flight_rotation(c, matrix);
    }
#else
    (void)c;
    (void)actor;
    (void)matrix;
#endif
}

/* The new-game opening, build e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348. Sequence/Opening.module.ainb requests
   Opening_Introduction and Opening_LoadingKokiriForest (SeqOneShotRequestEvent, f_80d9a820; the event name is the
   pointer at the request arg + 6800, which that function reads). Opening_Introduction's flowchart writes the state
   on both paths: the clock at 6:00, Meet_DekuTree's challenge Step01, the forest terrain pin, then
   EventTransition of the timeline Dm_Kokiri_0030. Its own skip query (IsEventSkipedMostRecent, f_80de6cb0) then
   takes the short edge: Navi warped to the position that edge names, the in-game music, Navi's talked flag, the
   camera handed back, and an autosave. Opening_LoadingKokiriForest's flowchart plays the timeline Dm_Kokiri_0000
   and then waits until the forest banc has loaded. Neither timeline writes save data. Meet_DekuTree is a later
   flow and is not one of these names.

   With Skip opening on, the request is still sent, so the flowchart applies that edge. The two timelines are not
   started: f_83b5bfb0 is their only starter (EventTransition at 0x80eaba00). Leaving bit 0 of the timeline player
   + 0xf25 clear makes that caller take its own exit at 0x80eac5e0, which finishes the action node, instead of the
   exit at 0x80eac640 that leaves the timeline running. The skip query is answered yes only while one of these two
   requests is the current one, so the short edge runs and a title demo that uses the same query does not. The
   cutscene frame counter is not moved. mode 0 observes a request, 1 a timeline start (return 1: do not start it),
   2 the skip query (return 1: x0 is 1), 3 Link is updating and no event is playing (the opening has let him go). */
int hos_oot_opening(Ctx *c, int mode) {
    int rx_option(int id, int otherwise);
    static int armed;
    static char logged[64];
    char name[80];
    uint64_t p = 0;
    if (mode == 3) {
        armed = 0;
        logged[0] = 0;
        return 0;
    }
    if (!c || rx_option(HOS_OPT_SKIP_OPENING, 0) <= 0) {
        armed = 0;
        return 0;
    }
    name[0] = 0;
    if (mode == 0) {
        const uint64_t arg = c->x[1];
        p = arg ? recomp_load64(c, arg + 6800) : 0;
    } else if (mode == 1 && c->x[1]) {
        p = recomp_load64(c, c->x[1]);   /* EventTransition passes a pointer to the timeline's name */
    }
    if (p) {
        size_t i = 0;
        for (; i + 1 < sizeof name; i++) {
            name[i] = (char)recomp_load8(c, p + i);
            name[i + 1] = 0;
            if (!name[i]) break;
        }
    }
    if (mode == 0) {
        if (!strcmp(name, "Opening_Introduction") || !strcmp(name, "Opening_LoadingKokiriForest")) {
            armed = 1;
            if (strcmp(logged, name)) {
                size_t i = 0;
                for (; name[i] && i + 1 < sizeof logged; i++) logged[i] = name[i];
                logged[i] = 0;
                hos_log("opening skip: %s request sent", name);
            }
        } else if (name[0]) {
            /* a different event (Meet_DekuTree included) is not skipped and drops the arm */
            armed = 0;
            logged[0] = 0;
        }
        return 0;   /* the request is still sent */
    }
    if (!armed) return 0;
    if (mode == 2) {
        c->x[0] = 1;
        return 1;
    }
    if (mode == 1 && (!strcmp(name, "Dm_Kokiri_0000") || !strcmp(name, "Dm_Kokiri_0030"))) {
        const uint64_t player = c->x[0];
        if (player) recomp_store8(c, player + 0xf25, recomp_load8(c, player + 0xf25) & ~(uint64_t)1);
        return 1;
    }
    return 0;
}

/* (longshot, issue 1113) HOS_INPUT_SYNC: Link put at a position through the game's own move of his character controller
   (OneShotPhysicsCctChangePosition, as the Mark / Return cheats do: oot_update_player_cheats), so his physics body moves
   with him; 0 when the controller or the move is not there */
#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
static int oot_put_link(Ctx *c, uint64_t components, const float pos[3]) {
    const uint64_t controller = oot_player_controller(c, components);
    if (!controller || recomp_load64(c, 0x84c54f10ULL + 0xe8) != 0x806d2530ULL) return 0;
    if (!game_has(0x806d2530ULL)) return 0;
    const uint64_t arg = c->x[31] - 16;
    for (unsigned i = 0; i < 3; i++) {
        uint32_t bits;
        memcpy(&bits, &pos[i], sizeof bits);
        recomp_store32_sp(c, arg + i * 4, bits);
    }
    recomp_store32_sp(c, arg + 12, 0);
    flight_pose_actor = 0;
    flight_transform_controller = 0;
    const GameCallMore lr = {.set_lr = 1, .lr = c->x[30]};   /* (LR as the update had it, as before) */
    game_call_more(c, 0x806d2530ULL, arg, 2, (const uint64_t[]){controller, arg}, NULL, &lr);
    return 1;
}
#endif

void hos_oot_lift_player(Ctx *c, uint64_t actor) {
#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
    flight_pose_actor = 0;
    if (recomp_load64(c, actor) != 0x84b45e18ULL) return;
    const uint64_t components = recomp_load64(c, actor + 624);
    const uint64_t movement = components ? recomp_load64(c, components + 800) : 0;
    if (movement < HOS_HEAP_BASE + 0x1d50 || recomp_load64(c, movement + 24) != actor ||
        !hos_oot_is_player(c, movement - 0x1d50)) return;
    if (!oot_event_playing(c)) hos_oot_opening(c, 3);   /* the opening has let Link go: later events keep their own skip */
    void hos_oot_note_link(uint64_t components);   /* game/oot_high_fps.c: his driven body's target above 60 fps (here, in the
                                                     debug tools' block, as the player's other hooks: built on both) */
    hos_oot_note_link(components);

    // Link can be updated by different guest workers on successive frames.
    // (Kept with the frame phases: the Debug tab's tools and the state tap still run here, on whichever job worker updates
    // Link, and the lock keeps two workers out of them at once. The acquire phase, game/phases.h, is the main thread's.)
    static SRWLOCK cheats_lock = SRWLOCK_INIT;
    if (!TryAcquireSRWLockExclusive(&cheats_lock)) return;
    oot_update_player_cheats(c, actor, components);
    void hos_oot_state_tap(uint64_t actor);   /* the control bridge's copied snapshot and per-connection samples */
    hos_oot_state_tap(actor);
    /* (longshot, issue 1113) an input recording's marks and anchors, and a replay held to them: Link's position (+0x31c, as
       the state tap reads it) after each update (host/input_movie.c); nothing read with nothing recorded or played */
    int hos_input_movie_active(void);
    if (hos_input_movie_active()) {
        int hos_input_movie_link(const float pos[3], float to[3]);
        float pos[3], to[3];
        for (int i = 0; i < 3; i++) {
            const uint32_t bits = (uint32_t)recomp_load32(c, actor + 0x31c + 4 * (uint64_t)i);
            memcpy(&pos[i], &bits, 4);
        }
        if (hos_input_movie_link(pos, to) && !oot_put_link(c, components, to)) {   /* (HOS_INPUT_SYNC: put back) */
            static int said;
            if (!said++) hos_log("input: HOS_INPUT_SYNC: Link's character controller was not found: he is not put back");
        }
    }
    ReleaseSRWLockExclusive(&cheats_lock);
#else
    (void)c;
    (void)actor;
#endif
}

#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
static void oot_build_free_view(Ctx *c, uint64_t view, uint64_t out) {
    int rx_option(int id, int otherwise);
    static uint64_t active_view;
    static int64_t last_tick;
    static double eye[3], yaw, pitch;
    const int64_t now = hos_now_ns();
    const int64_t elapsed = now - last_tick;

    if (!view) {
        active_view = 0;
        return;
    }
    if (active_view != view || elapsed > 500000000) {
        double target[3];
        memcpy(eye, A64_PTR(view + 96), sizeof eye);
        memcpy(target, A64_PTR(view + 120), sizeof target);
        const double x = eye[0] - target[0], y = eye[1] - target[1], z = eye[2] - target[2];
        yaw = atan2(x, z);
        pitch = atan2(y, hypot(x, z));
        active_view = view;
        hos_log("cheats: free camera at %.2f %.2f %.2f", eye[0], eye[1], eye[2]);
    }
    last_tick = now;
    DWORD foreground = 0;
    GetWindowThreadProcessId(GetForegroundWindow(), &foreground);
    const double dt = elapsed > 0 && elapsed < 100000000 ? (double)elapsed * 1e-9 : 0;
    const int control = foreground == GetCurrentProcessId() && !rx_option(1, 0);
    int height = 0;
    uint64_t hos_free_camera_input(int *height);
    const uint64_t input = control ? hos_free_camera_input(&height) : 0;
    if (control) {
        yaw -= 1.6 * dt * (int16_t)(input >> 32) / 32767.0;
        pitch -= 1.2 * dt * (int16_t)(input >> 48) / 32767.0;
        pitch = fmax(-1.55, fmin(1.55, pitch));
    }
    const double sy = sin(yaw), cy = cos(yaw), sp = sin(pitch), cp = cos(pitch);
    const double axes[3][3] = {{cy, 0, -sy}, {-sp * sy, cp, -sp * cy}, {cp * sy, sp, cp * cy}};
    if (control) {
        const double step = dt * (GetAsyncKeyState(VK_CONTROL) < 0 ? 30.0 : 8.0);
        const double right = (int16_t)input / 32767.0;
        const double forward = (int16_t)(input >> 16) / 32767.0;
        const int key_height = (GetAsyncKeyState(VK_PRIOR) < 0) - (GetAsyncKeyState(VK_NEXT) < 0);
        eye[1] += step * (key_height ? key_height : height);
        for (int i = 0; i < 3; i++)
            eye[i] += step * (right * axes[0][i] - forward * axes[2][i]);
    }
    for (int row = 0; row < 3; row++) {
        const double *axis = axes[row];
        const float values[4] = {(float)axis[0], (float)axis[1], (float)axis[2],
            (float)-(axis[0] * eye[0] + axis[1] * eye[1] + axis[2] * eye[2])};
        for (int col = 0; col < 4; col++) {
            uint32_t bits;
            memcpy(&bits, &values[col], sizeof bits);
            recomp_store32(c, out + (row * 4 + col) * 4, bits);
        }
    }
}
#endif

void hos_oot_free_view(Ctx *c, uint64_t view) {
#if defined(_WIN32) || defined(__linux__)   /* (the debug tools: host/posix has what they call) */
    int rx_option(int id, int otherwise);
    if (!view || recomp_load64(c, view) != 0x84c18a88ULL) return;

    // Camera zero supplies the gameplay view. The same matrix routine also serves other cameras.
    if (oot_gameplay_view(c) != view) return;

    static SRWLOCK lock = SRWLOCK_INIT;
    AcquireSRWLockExclusive(&lock);
    const int enabled = rx_option(HOS_OPT_CHEAT_CAMERA, 0);
    oot_build_free_view(c, enabled ? view : 0, view + 8);
    ReleaseSRWLockExclusive(&lock);
#else
    (void)c;
    (void)view;
#endif
}
