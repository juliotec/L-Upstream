/* What the host gives the game: the one header of host/ (and gpu/) that the game's C (game/) includes. The lift's runtime
   (a64.h: Ctx, the guest's registers and memory) is the other. tests/test_game_boundary.py checks both sides of the line:
   game/ includes nothing else of host/ or gpu/, and host/ and gpu/ reach the game only through game/game.h and
   game/options.h. game/README.md says what belongs on which side.

   It is what the game's code called before it moved out of host/, and nothing more. Each part stays the host's own
   header, re-exported here:
   - the game folder's paths (hos_paths.h: first, as it brings windows.h on Windows and host/posix's on Linux);
   - guest memory and Ctx, the guest window, logging, the clock, the host's allocations and its chains (hos.h);
   - calling the game's own functions (game_call.h: game_call, game_call_more);
   - the RomFS and its file readers (romfs.h), the resource size tables (restbl.h);
   - the settings file's path (options_path.h) and the options' values (rx_option, below: the game's ids in
     game/options.h, the others in gpu/vk/vk_api.h with HOS_MOUSE_*);
   - the switches' one rule (env_on: gpu/vk/compat/longshot_env.h);
   - the widescreen picture's size (hos_wide_size.h);
   - the input recording's clock and its menu choices (input_movie.h), the control bridge's watches (control.h);
   - and, declared below, the host's functions and values the game's code declared one by one where it used them:
     the options' changes, the frame's clock, input, the recordings, files, the zstd streams and the button prompts.
   The hook sites' flags and slots (game/sites.h) and the frame's phases (game/phases.h) are the game's own. */
#ifndef LONGSHOT_GAME_HOST_H
#define LONGSHOT_GAME_HOST_H

#include "hos_paths.h"
#include "hos.h"
#include "game_call.h"
#include "romfs.h"
#include "restbl.h"
#include "options_path.h"
#include "hos_wide_size.h"
#include "input_movie.h"
#include "control.h"
#include "../gpu/vk/vk_api.h"
#ifndef LONGSHOT_ENV_H
#include "../gpu/vk/compat/longshot_env.h"
#endif

#ifdef __cplusplus
extern "C" {
#endif
/* ---- the options (host/rx_backend.c) */
int rx_option(int id, int otherwise);   /* an option's value as the Insert menu has it, else otherwise */
void rx_option_set(int id, int value);  /* the game's change of one, which the menu shows */
int hos_dlss_jitter_clip(uint64_t projection, float out[2]);   /* the temporal upscalers' jitter, in the game's projection */

/* ---- the frame's clock and the picture (host/nn_vi.c, host/display.c, host/hos_main.c, host/hos_thread.c) */
extern volatile uint32_t hos_frame_seconds_bits;   /* the frame's seconds, a float's bits */
extern volatile uint64_t hos_step_probe;
extern volatile int64_t hos_present_count;
extern volatile int hos_fps_timing;
extern volatile int hos_scene_rgba16f;
extern volatile uint32_t hos_fixed_scale_bits;
extern volatile uint64_t hos_res_module;
extern volatile float hos_text_scale;
extern volatile float hos_camera_scale;   /* the Insert menu's Camera speed, 1.0 as shipped */
extern volatile int hos_fixed_resolution, hos_dynres_runs;
extern volatile uint64_t hos_wide_out, hos_wide_scene;   /* (host/hos_main.c: the wide screen's picture sizes) */
extern volatile float hos_wide_ratio, hos_wide_squeeze, hos_wide_margin;
extern volatile uint32_t hos_wide_keep_mask;
#ifndef __cplusplus
extern _Thread_local int64_t hos_slept_ns[2];   /* the FPS counter's Breakdown: the guest's sleeps (host/hos_thread.c) */
#endif
uint32_t hos_frame_whole(uint64_t at, uint32_t amount_bits);
uint32_t hos_frame_whole_times(uint64_t at, uint32_t amount_bits, uint32_t times);
uint32_t hos_loop_step(uint64_t at, uint32_t step_bits, uint32_t start_bits, uint32_t end_bits);
uint32_t hos_loop_pose(uint32_t time_bits, uint32_t start_bits, uint32_t end_bits);
uint32_t hos_movie_frames(uint64_t player);
void hos_movie_stats(int64_t out[5]);   /* (host/nn_video.c) */
int hos_camera_trace_enabled(void);
const char *hos_version(void);          /* (host/version.c) */

/* ---- input (host/nn_hid.c, host/display.c, host/mouse_native.c) */
extern volatile uint64_t hos_hid_buttons;
extern volatile int hos_oc_wait, hos_oc_last_kind;   /* the ocarina's note controls */
extern volatile unsigned hos_oc_beat;
void hos_hid_tap(const char *presses);
void hos_hid_stick(int lx, int ly, int rx, int ry, int hold_ms);
void hos_hid_drag(int dx, int dy, int right);
void hos_input_clear_toggles(void);
int hos_move_stick_held(void);
uint64_t hos_flight_input(void);
uint64_t hos_free_camera_input(int *height);
void hos_mouse_game_menu(void);
void hos_mouse_game_menu_gone(void);
int hos_mouse_game_pointer(int *x, int *y, int *w, int *h, uint32_t *pressed, int *wheel);
int hos_mouse_wheel_take(void);
int hos_disable_camera_centering(void);
void hos_mouse_camera(uint64_t cam);   /* the native mouse look, at the player camera's update */

/* ---- the input recordings (host/input_movie.c) */
void hos_input_movie_menu(int kind, uint64_t screen, uint64_t button, const char *key);
int hos_input_movie_menu_take(uint64_t screen, MovieMenu *out);
void hos_input_movie_menu_done(int met);
int hos_input_movie_playing(void);
int hos_input_movie_action(int live, int *dropped);
void hos_input_movie_action_taken(int a);
int hos_input_movie_active(void);
int hos_input_movie_link(const float pos[3], float to[3]);
int64_t hos_input_movie_time(HosMovieTime *t, int64_t wall_ns);   /* a clock that keeps a recording's time */

/* ---- files, the zstd streams and the button prompts (host/nn_fs.c, host/zs.c, host/hos_zstd.c, host/prompts.c, host/hos_mem.c) */
const char *hos_saves_root(void);   /* where the saves are kept: HOS_SAVE_DIR's folder, else the player's (host/save_dir.h) */
HANDLE hos_fs_open_read(const char *path);
const char *hos_fs_romfs_rel(const char *path);
int hos_fs_resolve_read(const char *guest, char *path, size_t cap);
uint8_t *hos_zs_read(const char *path, size_t *n, size_t limit);
uint64_t hos_zs_content_size(const char *path);
uint8_t *hos_zs_read_romfs(const char *rel, size_t *n, size_t limit);
uint8_t *hos_zs_read_romfs_named(const char *rel, size_t *n, size_t limit, const char *named);
uint64_t hos_zs_romfs_content_size(const char *rel);
int hos_zs_write(const char *path, const uint8_t *data, size_t n);
int hos_zstd_stream(Ctx *c);
int hos_zstd_frames(Ctx *c);
uint32_t hos_prompts_font_entry(uint32_t entry);
int hos_prompts_romfs(const char *key, const char *mod, char *out, size_t cap);
void hos_prompts_text_draw(uint64_t box);
void hos_prompts_text_layout(uint64_t box);
void hos_prompts_glyph(uint64_t slot);
int hos_mem_mapped(uint64_t addr, uint64_t n);

/* ---- main's image (gen/image.c, the lift's; hos.h has its build id) */
extern const uint64_t hos_main_base, hos_main_size, hos_main_code_end;

/* ---- guest memory for the game's Rust (host/game_guest.c: game/crates/guest calls these, a64.h's are inline) */
uint64_t hos_guest_end(void);                                       /* the guest window's end: an address below it is in it */
uint64_t hos_guest_load(Ctx *c, uint64_t a, unsigned size);         /* 1, 2, 4 or 8 bytes, as recomp_load* */
void hos_guest_store(Ctx *c, uint64_t a, unsigned size, uint64_t v);   /* as recomp_store*: its pages marked */
void *hos_guest_word(uint64_t a, unsigned size);                   /* an aligned 4- or 8-byte word's host address, or NULL */
void hos_guest_mark(uint64_t a, unsigned size);                     /* its pages marked after an atomic write */
int hos_guest_wait32(uint64_t a, uint32_t expected, uint32_t ms);   /* sleep while the word is expected (WaitOnAddress) */
void hos_guest_wake32(uint64_t a, int all);                         /* wake one sleeper on it, or all */
#ifdef __cplusplus
}
#endif

#endif
