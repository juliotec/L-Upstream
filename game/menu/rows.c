/* The game's rows of the Insert menu: rows of game features, as data (game/game.h GameMenuRow). The renderer adds them to
   its menu at the start, in place (gpu/vk/menu/game_rows.cpp), keeps their values by their option ids (game/options.h),
   saves them under their keys in longshot_options.ini and answers rx_option for them. A key never changes: it is the
   player's setting. The texts are the menu's text ids (gpu/vk/menu_i18n.h translates them): a row moved keeps them to the
   character. */
#include "../game.h"

/* ---- the Gameplay tab: how the game plays and starts */
static const char *const text_speeds[] = {"Normal", "2x", "4x", "Instant"};
static const char *const text_speed_hints[] = {"The game's own pace, with its pauses.", "Twice as fast.", "Four times as fast.",
                                               "Each page at once, as pressing A does."};
static const char *const text_sizes[] = {"Original", "110%", "125%", "150%", "175%"};
static const char *const text_size_hints[] = {"The game's original size.", "A little larger.", "Larger.", "Much larger.", "Largest."};

/* ---- the Graphics tab's Screen section: where the game's menus sit on a screen that is not 16:9 (game/oot_wide_sites.c,
   game/hos_wide_ui.c) */
static const char *const wide_menus[] = {"Full width", "Centred, blurred sides", "Centred, black bars", "Extended"};
static const char *const wide_menus_hints[] = {
    "The menus stretched over the whole screen, as before.",
    "The menus in the middle 16:9; beside them, what the game draws there, out of focus.",
    "The menus in the middle 16:9; black beside them.",
    "The menus at their 16:9 size, what sits at their sides moved out to the screen's edges."};

static const GameMenuRow rows[] = {
    {.key = "field_of_view",
     .label = "Field of view",
     .help = "How much of the world you see: 100% is the original view. Higher is wider, lower is closer. Changes apply at once "
             "during gameplay. Cutscenes keep their own view.",
     .tab = "gameplay", .kind = GAME_MENU_SLIDER, .option = HOS_OPT_FOV,   /* (game/crates/oot/src/fov.rs) */
     .lo = 50, .hi = 150, .step = 5, .default_value = 100,
     .text = GAME_MENU_TEXT_PERCENT, .default_text = "100% (original)",
     .before = "bug_report", .order = 310, .preset_exempt = 1},
    {.key = "text_speed",
     .label = "Text speed",
     .help = "How fast the letters of dialogue and other messages appear.",
     .tab = "gameplay", .kind = GAME_MENU_CHOICE, .option = HOS_OPT_TEXT_SPEED,   /* (host/nn_vi.c, game/oot_text_step.c) */
     .lo = 0, .hi = 3, .step = 1, .default_value = 0, .choices = text_speeds, .hints = text_speed_hints,
     .before = "bug_report", .order = 320, .preset_exempt = 1},
    {.key = "text_size",
     .label = "Text size",
     .help = "How large the text of dialogue, Navi and signs is drawn.",
     .tab = "gameplay", .kind = GAME_MENU_CHOICE, .option = HOS_OPT_TEXT_SIZE,   /* (game/oot_text_size.c) */
     .lo = 0, .hi = 4, .step = 1, .default_value = 0, .choices = text_sizes, .hints = text_size_hints,
     .before = "bug_report", .order = 330, .preset_exempt = 1},
    {.key = "skip_title_screen",
     .label = "Skip title screen",
     .help = "Starts a launch with the most recent saved game: presses A on the title screen, its file list and its Yes as "
             "soon as the game waits on each. Does nothing until the game has a save. Applies after a restart.",
     .tab = "gameplay", .kind = GAME_MENU_TOGGLE, .option = HOS_OPT_SKIP_TITLE,   /* (game/oot_autoload.c) */
     .lo = 0, .hi = 1, .step = 1, .default_value = 0,
     .before = "bug_report", .order = 340, .restart = 1, .preset_exempt = 1},
    {.key = "discord_status",
     .label = "Discord status",
     .help = "Shows \"Playing Longshot\" and the time played in your Discord status, while Discord runs on this PC.",
     .tab = "gameplay", .kind = GAME_MENU_TOGGLE, .option = HOS_OPT_DISCORD,   /* (game/discord_rpc.c) */
     .lo = 0, .hi = 1, .step = 1, .default_value = 1,
     .before = "bug_report", .order = 500, .preset_exempt = 1},
    {.key = "skip_opening",
     .label = "Skip opening",
     .help = "Skips the new-game opening (the story movie and Navi waking Link) and leaves later cutscenes, the Deku Tree "
             "meeting, and the title screen alone.",
     .tab = "gameplay", .kind = GAME_MENU_TOGGLE, .option = HOS_OPT_SKIP_OPENING,   /* (game/oot_tools.c) */
     .lo = 0, .hi = 1, .step = 1, .default_value = 0,
     .before = "bug_report", .order = 600, .preset_exempt = 1},
    {.key = "ultrawide_menus",
     .label = "Ultrawide menus",
     .help = "With the Aspect ratio at the monitor's shape: the pause, item and map menus, and what shows beside them. Loading "
             "screens and pre-rendered movies, which draw no world either, get the same sides. Changes at once.",
     .tab = "graphics", .kind = GAME_MENU_CHOICE, .option = HOS_OPT_WIDE_MENUS,
     .lo = 0, .hi = 3, .step = 1, .default_value = 1, .choices = wide_menus, .hints = wide_menus_hints,
     .after = "fullscreen_monitor", .order = 50, .preset_exempt = 1},
};

const GameMenuRow *game_menu_rows(int *count) {
    *count = (int)(sizeof rows / sizeof rows[0]);
    return rows;
}
