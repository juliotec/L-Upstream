/* The option ids of the game's own features: the numbers its code reads with rx_option (game_host.h) and the Insert
   menu's rows save. The renderer and the host keep theirs in gpu/vk/vk_api.h, which includes this file: one space of ids,
   answered by one option callback. An id's value never changes: longshot_options.ini and the input recordings hold them.
   Header only, C and C++. */
#ifndef LONGSHOT_GAME_OPTIONS_H
#define LONGSHOT_GAME_OPTIONS_H

/* the Gameplay tab's Text speed (host/nn_vi.c reads it as 5: the letters' clock, game/oot_text_step.c): 0 the game's, 1 2x,
   2 4x, 3 instant */
enum { HOS_OPT_TEXT_SPEED = 5 };
/* (longshot) the Gameplay tab's Text size (game/oot_text_size.c): 0 the game's, 1 110%, 2 125%, 3 150%, 4 175% */
enum { HOS_OPT_TEXT_SIZE = 10 };
/* The Debug tab's cheats (game/oot_tools.c): Infinite hearts, Flight, the action, Free camera, Movement speed */
enum { HOS_OPT_CHEAT_HEARTS = 32, HOS_OPT_CHEAT_FLIGHT, HOS_OPT_CHEAT_ACTION, HOS_OPT_CHEAT_CAMERA, HOS_OPT_CHEAT_SPEED };
/* (HOS_OPT_DISCORD, 43: the Insert menu's Discord status row, game/discord_rpc.c: 1 on; HOS_OPT_SKIP_TITLE, 44: its Skip
   title screen row, game/oot_autoload.c: 1 on. 40 to 42 are the renderer's HDR rows: gpu/vk/vk_api.h) */
enum { HOS_OPT_DISCORD = 43, HOS_OPT_SKIP_TITLE = 44 };
/* Gameplay's vertical field of view, percent of the game's current angle; 100 is unchanged. */
/* Walk mode and Walk speed use 49 and 50. */
enum { HOS_OPT_FOV = 51 };
/* (HOS_OPT_SKIP_OPENING, 52: Gameplay's Skip opening, game/oot_tools.c hos_oot_opening. Off. The new-game story movie
   and Navi waking Link. 44 stays reserved for Skip title, 49 and 50 are Walk's, 60 is reserved for the item wheel. A new option id
   answered by the existing option callback does not change MkwVkHost or MkwVkRenderer.) */
enum { HOS_OPT_SKIP_OPENING = 52 };
/* Damage taken: 0 Half, 1 Normal (unchanged), 2 Double, 3 Quadruple. */
enum { HOS_OPT_DAMAGE_TAKEN = 55 };
/* (HOS_OPT_SHADOW_RES, 56: Graphics' Shadow resolution, game/oot_shadow.c: the sun's cascades as drawn, its Auto resolved
   (gpu/vk/vk_menu.cpp menu_shadow_res), 0 the game's 1600, 1 2200, 2 3072, read each frame. The existing option callback:
   no new member.) */
enum { HOS_OPT_SHADOW_RES = 56 };
/* Transition autosave (game/oot_tools.c hos_oot_trace_save): 1, the game's, a save as a map is crossed; 0 none */
enum { HOS_OPT_TRANSITION_AUTOSAVE = 59 };
/* (HOS_OPT_HUD_LAYOUT, 71: Graphics' HUD layout, game/hos_wide_ui.c reads it at each present by its number: on a screen
   that is not 16:9, 0 the HUD's corners at the screen's edges, 1 the HUD in the middle 16:9 as before #95 (on a narrower
   screen fitted to its width, as the menus). HOS_WIDE_UI, when set, stands over it. The existing option callback: no new
   member. 62 to 70, 80, 88 and 90 are taken by open pull requests.) */
enum { HOS_OPT_HUD_LAYOUT = 71, HOS_OPT_HUD_HEALTH, HOS_OPT_HUD_BUTTONS, HOS_OPT_HUD_MINIMAP };
/* One packed placement per group: x and y in percent of the picture (signed, biased by 16), scale in percent. */
enum { HOS_HUD_DEFAULT_PLACEMENT = 16 | (16 << 6) | (100 << 12) };
/* (HOS_OPT_WIDE_MENUS, 102: Graphics' Ultrawide menus, game/hos_wide_ui.c reads it at each present by its number: 3 is Extended,
   the menus at their 16:9 size with their side panes at the screen's edges. The existing option callback: no new member.) */
enum { HOS_OPT_WIDE_MENUS = 102, HOS_WIDE_MENUS_EXTENDED = 3 };

#endif
