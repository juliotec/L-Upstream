/* The player's settings, longshot_options.ini's lines (host/options_path.c): the profile's settings.ini (LONGSHOT_PROFILE,
   which longshot-builder's play sets), else beside the game as before. Paths are UTF-8. */
#pragma once
/* Chooses the file from LONGSHOT_PROFILE: at the very start of main, before anything reads it (the crash window's helper,
   the Discord status, the renderer). Nothing is moved or made; once a process */
void hos_options_choose(void);
/* The file's path: "" before hos_options_choose or when none could be chosen (no file: every setting at its default) */
const char *hos_options_file(void);
/* The file's path into the log ("options: the settings file is <path>"), and the game folder's longshot_options.ini, a build's
   from before, copied into the profile once: when the profile has none and it holds a setting (not only longshot-builder's
   compile_shaders=). Once the log is open */
void hos_options_carry(void);
