/* What the game gives the host and the renderer: the one header of game/ that host/ and gpu/ include (with
   game/options.h, the game's option ids). The other side of the line is host/game_host.h, what the host gives the game;
   tests/test_game_boundary.py checks both, and game/README.md says what belongs on which side.

   Each hook is a plain C function, called where the host called the game's code before it moved to game/. Names that
   said Ocarina of Time (hos_oot_*) are generic here (game_*), so the host and the renderer name no oot_ symbol; the
   game's other hooks keep the hos_ names they had (hos_cheats_frame, hos_mod_info, hos_sprint_input, ...). A table of
   function pointers is only where the host keeps one already (the control bridge's HosControlGame, host/control.h).
   C and C++. */
#ifndef LONGSHOT_GAME_GAME_H
#define LONGSHOT_GAME_GAME_H

#include <stddef.h>
#include <stdint.h>
#include "options.h"   /* (the game's option ids) */

#ifdef __cplusplus
extern "C" {
#endif
#include "phases.h"    /* (GamePhase, game_phase_run: the points of the game's frame where host code runs) */

struct HosControlGameWatch;   /* (host/control.h) */

/* ---- the start (host/hos_main.c) */
void game_init(void);                       /* the game's switches (game/oot_tools.c), before the renderer starts */
void hos_game_layout(void);                 /* the game folder's cache/ and logs/, before anything writes there (game_layout.c) */
void hos_game_layout_log(void);             /* what it moved or made, once the log is open */
void hos_discord_start(void);               /* the Discord status (discord_rpc.c) */
void hos_mods_init(const char *image_path); /* the mods that are on (mods.c) */
void hos_cheats_init(void);                 /* the cheats of the mods that are on (cheats.c) */

/* ---- the frame */
/* game_phase_run (phases.h, host/nn_vi.c at the window acquire): a frame phase's callbacks */
void hos_cheats_frame(void);                /* (host/nn_vi.c, each present) the cheats' writes (cheats.c) */
void hos_wide_ui_tick(void);                /* (host/rx_backend.c, each present) the HUD's layout for the picture's shape */

/* ---- input (host/nn_hid.c) */
extern volatile int game_live_action;       /* (test runs) the live input file's "!cheat <action>" (oot_tools.c) */
int game_flight_shortcut_ready(void);       /* Flight's shortcut may toggle: Link seen and no event */
void game_autoload_input(void);             /* Skip title screen's presses, each input poll (oot_autoload.c) */
void hos_gameplay_hud_toggle(void);         /* the HUD shown or hidden (hos_wide_ui.c) */
/* Sprint (oot_sprint.c): the buttons the game gets, with Sprint's dash held, toggled or let go */
uint64_t hos_sprint_input(uint64_t buttons, int on, int down, int l3, int toggle, int reset, int blocked);

/* ---- the control bridge's game commands (host/control.c) and the input recordings' (host/input_movie.c) */
int game_current_map(char *out, size_t cap);                          /* the map Link is in, and his age */
int game_state(int *event, int *loading, int *child);                 /* an event playing, a map loading, the child */
int game_destination(int index, const char **child, const char **adult);   /* a teleport's destination by index */
int game_teleport_ask(int index);                                     /* a teleport asked for: its ticket, or 0 */
int game_teleport_result(int ticket, char *why, size_t why_cap, char *map, size_t map_cap, int *accepted);
void game_teleport_cancel(int ticket);
int game_age_ask(int adult);                                          /* the age asked for: a ticket, as a teleport's */
uint64_t game_sample_ask(void);                                       /* the state tap (oot_state_tap.c) */
void game_sample_cancel(uint64_t ticket);
int game_sample(uint64_t ticket, char *out, size_t cap);
void game_watch(int slot, const struct HosControlGameWatch *watch);
int game_poll(int slot, char *out, size_t cap);

/* ---- the renderer's table (host/rx_backend.c's MkwVkHost): the Insert menu's Mods and Cheats pages, the performance summary */
int hos_mod_info(int i, const char **name, const char **detail, const char **note, int *enabled);
void hos_mod_edit(int i, int op);
uint64_t hos_mods_changed(void);
int hos_cheat_info(int i, const char **name, const char **mod, const char **note, int *kind, int *on);
void hos_cheat_set(int i, int on);
const char *hos_perf_info(char *map, int cap, int64_t movie[5], float pose[5], int *has_pose);
void hos_perf_tracking(int enabled);

/* ---- files (host/nn_fs.c, host/prompts.c): what the mods replace, and the stand-ins for damaged files */
int hos_mods_romfs(const char *rel, char *out, size_t cap);
int hos_mods_file(const char *rel, char *out, size_t cap);
int hos_mods_dir_extras(const char *rel, int all, void (*fn)(void *, const char *, int, uint64_t), void *arg);
int hos_stand_in_damaged(const char *guest, char *path, size_t cap);

/* ---- the Insert menu's rows of the game (game/menu/): plain data, which the renderer adds to its menu at the start, in
   place, as a module of gpu/vk/menu/ adds its own (gpu/vk/menu/game_rows.cpp, through MkwVkHost's game_menu_rows). A row is
   the game's when it changes the game, not the picture. The renderer keeps each row's value by its option id, saves it
   under its key in longshot_options.ini and answers rx_option for it. A row that needs the renderer's code (ImGui, hooks)
   is the renderer's, in gpu/vk/menu/. */
enum GameMenuKind { GAME_MENU_CHOICE, GAME_MENU_TOGGLE, GAME_MENU_SLIDER };
enum GameMenuText { GAME_MENU_TEXT_NUMBER, GAME_MENU_TEXT_PERCENT };
typedef struct GameMenuRow {
    const char *key;      /* its line in longshot_options.ini and its name over the control bridge: never changes */
    const char *label;    /* (label, help, choices and hints are the menu's text ids, its English text: a row moved keeps
                             them to the character) */
    const char *help;
    const char *tab;      /* the tab it is on, by its id ("graphics", "gameplay": gpu/vk/vk_menu.cpp kTabIds) */
    int kind;             /* GAME_MENU_*: a Choice steps through its values and wraps; a Toggle is off or on; a Slider stops at its ends */
    int option;           /* its option id (game/options.h): the value the game reads with rx_option */
    int lo, hi, step;
    int default_value;    /* (out of the box) */
    const char *const *choices;   /* a Choice's names, lo first, hi - lo + 1 of them; NULL: by text */
    const char *const *hints;     /* a line about each value, lo first, hi - lo + 1 of them (NULL for a value without); NULL none */
    int text;             /* a value's text without choices: GAME_MENU_TEXT_NUMBER, or PERCENT ("%d%%"); a Toggle's: On, Off */
    const char *default_text;     /* the default value's own text, over text ("100% (original)"); NULL none */
    const char *after;    /* where it goes: right after the row (or section) of this key, or before this one; NULL none */
    const char *before;
    int order;            /* among the rows placed beside the same key, by this */
    int restart;          /* a change waits for a restart */
    int preset_exempt;    /* the Quality preset leaves it alone */
} GameMenuRow;
/* The game's rows (game/menu/rows.c), *count of them */
const GameMenuRow *game_menu_rows(int *count);

#ifdef __cplusplus
}
#endif

/* ---- the Debug tab's inventory items (the menu's Inventory page draws them, oot_tools.c edits them): OOT_CHEAT_ITEMS */
#include "oot_cheat_items.h"

#endif
