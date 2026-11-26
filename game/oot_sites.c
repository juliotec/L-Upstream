/* (longshot) The engine's callbacks at the game's hook sites (game/sites/sites.toml, game/sites.def), where the
   feature itself lives in another file: each is what the old builder's text patch it replaced put at that place,
   with the answer the site's form reads (docs/dev/reference/hooks.md). */
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include "sites.h"
#include <string.h>

/* ---- build e0f3ccfa's addresses the callbacks read (docs/dev/reference/hooks.md: a callback naming an address belongs to that build) */
#define OOT_TIME_SLOT (0x80435000ULL + (int64_t)75927552 + 2192)   /* (the time object's pointer: 0x84c9e890) */
#define OOT_CLOTH_CLOCK (0x84d61fc8ULL)   /* (the cloth's clock word) */
#define OOT_SITES_BUILD "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348"   /* (the build these values are of) */
#define OOT_BUTTON_WAIT_SCREEN 0xe8   /* (a screen button wait node's ScreenName input: [[node + 0xe8] & ~3] its name) */
/* ---- end of the build's addresses */

int hos_oot_opening(Ctx *c, int mode);   /* (game/oot_tools.c: Skip opening) */

/* opening.request (entry): mode 0 observes the request; the function goes on */
int hos_site_opening_request(Ctx *A64_NOESC c) {
    hos_oot_opening(c, 0);
    return 0;
}

/* opening.timeline (entry, can_return): non-zero leaves the timeline unstarted */
int hos_site_opening_timeline(Ctx *A64_NOESC c) { return hos_oot_opening(c, 1); }

/* opening.skipQuery (entry, can_return): non-zero answers the query (x0 set by hos_oot_opening) */
int hos_site_opening_skip_query(Ctx *A64_NOESC c) { return hos_oot_opening(c, 2); }

int hos_oot_autoload_watching(void);   /* (game/oot_autoload.c: Skip title screen waits for the title) */
void hos_oot_autoload_wait(const char *screen);

/* menu.buttonWait and menu.buttonWaitNoDecideAnim (entry): a sequence node starts waiting for a press of a screen's button
   (x0 the node). While Skip title screen waits for the title, the screen's name is read as the function reads it next, and
   told; otherwise nothing is read. The function goes on */
int hos_site_menu_button_wait(Ctx *A64_NOESC c) {
    if (!hos_oot_autoload_watching()) return 0;
    char name[48];
    size_t n = 0;
    const uint64_t input = recomp_load64(c, c->x[0] + OOT_BUTTON_WAIT_SCREEN) & ~3ULL;
    const uint64_t text = input ? recomp_load64(c, input) : 0;
    for (; text && n + 1 < sizeof name; n++)
        if (!(name[n] = (char)recomp_load8(c, text + n))) break;
    name[n] = 0;
    hos_oot_autoload_wait(name);
    return 0;
}

/* Whether the build running has the two sites, so Skip title screen hears the game's button waits: this build is the one
   the callback's offset is of */
int hos_oot_button_wait_sites(void) {
    /* (its table, game/sites/sites.toml, has menu.buttonWait and menu.buttonWaitNoDecideAnim, and a lift
       that cannot place a site of the table stops: builder/crates/sites resolve. So a game of this build has both) */
    return !strcmp(hos_main_build_id, OOT_SITES_BUILD);
}

int hos_oot_trace_save(Ctx *c);   /* (game/oot_tools.c) */

/* save.request (entry, can_return): non-zero drops the request, x0 = 0 set by hos_oot_trace_save */
int hos_site_save_request(Ctx *A64_NOESC c) { return hos_oot_trace_save(c); }

extern volatile float hos_camera_scale;   /* (host/nn_vi.c: the Insert menu's Camera speed, 1.0 as shipped) */

/* camera.yawSpeed and the three others (return): s0, the turn speed the function returns, times the menu's scale; at 100%
   the register as the game left it */
int hos_site_camera_turn_speed(Ctx *A64_NOESC c) {
    if (hos_camera_scale != 1.0f) {
        float t;
        uint32_t u;
        memcpy(&t, c->vreg[0], 4);
        t *= hos_camera_scale;
        memcpy(&u, &t, 4);
        c->vreg[0][0] = u;   /* (a write of s0: the rest of v0 zeroed, as the lift writes it) */
        c->vreg[0][1] = 0;
    }
    return 0;
}

void hos_mouse_camera(uint64_t cam);   /* (host/mouse_native.c: native mouse look) */

/* camera.update (entry): the player camera, before its update reads anything */
int hos_site_mouse_camera(Ctx *A64_NOESC c) {
    hos_mouse_camera(c->x[0]);
    return 0;
}

void hos_oot_menu_focus(Ctx *c, uint64_t screen, uint64_t pane, uint64_t lr);   /* (game/oot_menu.c: the menus' mouse) */
void hos_oot_menu_tick(Ctx *c, uint64_t screen);

/* menu.focus (entry): the screen, the pane and the caller's return address, before the handler runs */
int hos_site_menu_focus(Ctx *A64_NOESC c) {
    hos_oot_menu_focus(c, c->x[0], c->x[1], c->x[30]);
    return 0;
}

/* menu.update (entry): the screen, before its update runs */
int hos_site_menu_update(Ctx *A64_NOESC c) {
    hos_oot_menu_tick(c, c->x[0]);
    return 0;
}

void hos_prompts_text_draw(uint64_t box);   /* (host/prompts.c: the button prompts changed while the game runs) */
void hos_prompts_text_layout(uint64_t box);
void hos_prompts_glyph(uint64_t slot);

/* text.boxDraw (entry): the text box, before the game draws it */
int hos_site_text_box_draw(Ctx *A64_NOESC c) {
    hos_prompts_text_draw(c->x[0]);
    return 0;
}

/* text.boxLayout (entry): the text box, before its layout looks at its status */
int hos_site_text_box_layout(Ctx *A64_NOESC c) {
    hos_prompts_text_layout(c->x[0]);
    return 0;
}

/* font.glyphRender (entry): FreeType's glyph slot, before ft_smooth_render reads it */
int hos_site_glyph_render(Ctx *A64_NOESC c) {
    hos_prompts_glyph(c->x[1]);
    return 0;
}

int hos_zstd_stream(Ctx *c);   /* (host/hos_zstd.c: 1 when it did the call, x0 its result) */
int hos_zstd_frames(Ctx *c);

/* zstd.stream (entry, can_return): non-zero, the chunk was decoded by libzstd and the function returns with x0 */
int hos_site_zstd_stream(Ctx *A64_NOESC c) { return hos_zstd_stream(c); }

/* zstd.frames (entry, can_return): the same for the whole-buffer decode */
int hos_site_zstd_frames(Ctx *A64_NOESC c) { return hos_zstd_frames(c); }

int hos_oot_cloth_solve(uint32_t step_bits);   /* (game/oot_high_fps.c: HOS_FPS_CLOTH_60) */

/* cloth.solve (entry, can_return): the game's frame step (the float f_83b6ea90 writes at 0x84d61fc8) to the gate; a frame it
   holds returns at once with the job's own result, 0 */
int hos_site_cloth_solve(Ctx *A64_NOESC c) {
    if (hos_oot_cloth_solve((uint32_t)recomp_load32(c, OOT_CLOTH_CLOCK))) return 0;
    c->x[0] = 0;
    return 1;
}

uint32_t hos_movie_frames(uint64_t player);   /* (host/nn_vi.c: 1 while the host gives no frame time) */

/* movie.frameCount (replace add w8, w8, #1): the count steps by the frames' time in 60ths, a 32-bit add as the game's */
int hos_site_movie_frame_count(Ctx *A64_NOESC c) {
    c->x[8] = (uint32_t)(c->x[8] + hos_movie_frames(c->x[20]));
    return 0;
}

extern volatile int hos_scene_rgba16f;   /* (host/nn_vi.c: Native HDR with the PsychoV35 tone mapper) */

/* render.sceneFormat (replace mov w9, #0x1a): RGBA16F (44) in place of R11G11B10F (26) while the switch is on */
int hos_site_scene_format(Ctx *A64_NOESC c) {
    c->x[9] = hos_scene_rgba16f ? 0x2cULL : 0x1aULL;
    return 0;
}

uint32_t hos_oot_damage(uint32_t old, uint32_t next, uint32_t hit);   /* (game/oot_damage.c) */
int hos_oot_is_player(Ctx *c, uint64_t actor);   /* (game/oot_tools.c) */

/* A life write that lowers the player's own life (x19 the actor, x13 its life field: the pointer at +7184, plus 8) takes the
   host's new life in w12: Damage taken's scale, or the old life under heart protection */
static void life_write(Ctx *A64_NOESC c, uint32_t hit) {
    if ((uint32_t)c->x[12] < (uint32_t)c->x[11] && hos_oot_is_player(c, c->x[19]) &&
        c->x[13] == recomp_load64(c, c->x[19] + 7184) + 8)
        c->x[12] = hos_oot_damage((uint32_t)c->x[11], (uint32_t)c->x[12], hit);
}

/* player.lifeHit (before the damage's cas): the hit is x9, whole, before the game clamped the life at 0 */
int hos_site_life_hit(Ctx *A64_NOESC c) {
    life_write(c, (uint32_t)c->x[9]);
    return 0;
}

/* player.lifeWrite (before the other cas): the hit is what the write takes off */
int hos_site_life_write(Ctx *A64_NOESC c) {
    life_write(c, (uint32_t)c->x[11] - (uint32_t)c->x[12]);
    return 0;
}

void hos_oot_lift_player(Ctx *c, uint64_t actor);   /* (game/oot_tools.c: Link's update, the debug tools' flight) */
void hos_oot_flight_pose(Ctx *c, uint64_t actor, uint64_t matrix);
void hos_oot_flight_rotation(Ctx *c, uint64_t controller, uint64_t matrix);

/* player.update (before the movement update's tbnz w8, #1): the actor */
int hos_site_player_update(Ctx *A64_NOESC c) {
    hos_oot_lift_player(c, c->x[19]);
    return 0;
}

/* player.pose (before +0x198, where the update's two paths meet): the actor and its pose matrix */
int hos_site_player_pose(Ctx *A64_NOESC c) {
    hos_oot_flight_pose(c, c->x[19], c->x[25]);
    return 0;
}

/* player.flightRotation (before the blr): the controller and its matrix */
int hos_site_flight_rotation(Ctx *A64_NOESC c) {
    hos_oot_flight_rotation(c, c->x[0], c->x[1]);
    return 0;
}

void hos_oot_free_view(Ctx *c, uint64_t view);   /* (game/oot_tools.c: the free camera) */

/* camera.view (before the head of the views' loop): the view, each time round */
int hos_site_camera_view(Ctx *A64_NOESC c) {
    hos_oot_free_view(c, c->x[20]);
    return 0;
}

void hos_oot_cct_speed(Ctx *c, uint64_t controller, int per_second);   /* (game/oot_tools.c: Movement speed) */

/* player.speedMove and player.speedMoveOther (after Link's move takes the controller): s0 in metres a frame */
int hos_site_speed_per_frame(Ctx *A64_NOESC c) {
    hos_oot_cct_speed(c, c->x[0], 0);
    return 0;
}

/* player.speedSet (the setter's entry) and player.speedSetComponent (after the component's controller is taken): s0 in metres
   a second */
int hos_site_speed_per_second(Ctx *A64_NOESC c) {
    hos_oot_cct_speed(c, c->x[0], 1);
    return 0;
}

int hos_oot_horse_whole_step(uint32_t step_bits, uint64_t slot);   /* (game/oot_high_fps.c) */

/* horse.jumpStep (branch, before the b.pl past the slow-motion branch): taken when the step is the whole frame's. The slot is
   the guest address of the pointer to the game's time object (its +0x10 the frame's step), as the patches named it */
int hos_site_horse_jump_step(Ctx *A64_NOESC c) {
    return hos_oot_horse_whole_step((uint32_t)c->vreg[10][0], OOT_TIME_SLOT);
}

int hos_oot_tex_compact_due(void);   /* (game/oot_tex_compact.c) */

/* texture.compactAsk (branch, before tbz w8, #1 past the ask): taken (no ask) when the flag is set but the host says not now;
   with the flag clear the tbz's own test takes it, and the host is not asked, as the patch's || had it */
int hos_site_tex_compact_ask(Ctx *A64_NOESC c) { return ((c->x[8] >> 1) & 1) && !hos_oot_tex_compact_due(); }

uint32_t hos_oot_solver_steps(uint32_t game, uint32_t scaled_bits, uint32_t per_frame_bits);   /* (game/oot_high_fps.c) */

/* physics.solverSteps and physics.solverStepsOther (after fcvtzs w1, s0): w1 the host's count, the game's at a cap of 60 */
int hos_site_solver_steps(Ctx *A64_NOESC c) {
    c->x[1] = hos_oot_solver_steps((uint32_t)c->x[1], (uint32_t)c->vreg[0][0], (uint32_t)c->vreg[1][0]);
    return 0;
}

int hos_oot_sound_gate(Ctx *c, uint32_t phase, uint64_t slot);   /* (game/oot_high_fps.c: the sound job at 60 Hz) */
uint32_t hos_oot_sound_step(uint32_t step_bits);
uint32_t hos_oot_sound_seconds(uint32_t seconds_bits, uint32_t step_bits);
void hos_oot_bgm_begin(void);
uint64_t hos_oot_bgm_flags(uint64_t at, uint64_t flags);

/* sound.job (entry, can_return): the gate holds the job (non-zero) or runs it. The slot is the guest address of the pointer to
   the game's time object, as the patches named it */
int hos_site_sound_job(Ctx *A64_NOESC c) {
    return !hos_oot_sound_gate(c, (uint32_t)c->x[2], OOT_TIME_SLOT);
}

/* sound.jobStep (after ldr s0, [x8, #0x10]): the step the engine gets, a whole one while the gate holds the job */
int hos_site_sound_job_step(Ctx *A64_NOESC c) {
    c->vreg[0][0] = hos_oot_sound_step((uint32_t)c->vreg[0][0]);
    return 0;
}

/* music.update (entry): the change flags held back since its last run are this run's */
int hos_site_music_update(Ctx *A64_NOESC c) {
    (void)c;
    hos_oot_bgm_begin();
    return 0;
}

/* music.time (after ldr s9, [x8, #0x18]): its step and its seconds, the seconds with the step as the game wrote it */
int hos_site_music_time(Ctx *A64_NOESC c) {
    c->vreg[8][0] = hos_oot_sound_step((uint32_t)c->vreg[8][0]);
    c->vreg[9][0] = hos_oot_sound_seconds((uint32_t)c->vreg[9][0], (uint32_t)recomp_load32(c, c->x[8] + 16));
    return 0;
}

/* music.changeFlags and the three after it (after ldrb w8, [x26, #0x898]): the byte with the flags the gate held back */
int hos_site_music_change_flags(Ctx *A64_NOESC c) {
    c->x[8] = hos_oot_bgm_flags(c->x[26] + 2200, c->x[8]);
    return 0;
}

void hos_oot_drive_target(uint64_t controller, uint64_t body, uint64_t slot);   /* (game/oot_high_fps.c) */

/* physics.driveTarget (after ldr x23, [x20, #0xf0]): the controller and its body, before the target is read; the slot is the
   guest address of the pointer to the game's time object */
int hos_site_drive_target(Ctx *A64_NOESC c) {
    hos_oot_drive_target(c->x[20], c->x[23], OOT_TIME_SLOT);
    return 0;
}

/* physics.driveTargetOther (after ldr x24, [x20, #0xf0]): the same on the path's other copy */
int hos_site_drive_target_other(Ctx *A64_NOESC c) {
    hos_oot_drive_target(c->x[20], c->x[24], OOT_TIME_SLOT);
    return 0;
}

uint32_t hos_oot_cloth_seconds(uint32_t seconds_bits, uint32_t step_bits);   /* (game/oot_high_fps.c) */

/* cloth.time (after ldr d0, [x22, #8]): its low half the host's time, by the game's frame step (the float at 0x84d61fc8); the
   high half as loaded */
int hos_site_cloth_time(Ctx *A64_NOESC c) {
    const uint64_t v = c->vreg[0][0];
    c->vreg[0][0] = (v & 0xffffffff00000000ULL) |
                    hos_oot_cloth_seconds((uint32_t)v, (uint32_t)recomp_load32(c, OOT_CLOTH_CLOCK));
    return 0;
}

uint32_t hos_frame_whole(uint64_t at, uint32_t amount_bits);   /* (host/nn_vi.c: the step's whole frames) */
uint32_t hos_frame_whole_times(uint64_t at, uint32_t amount_bits, uint32_t times);

/* frame.whole* (replace the cut of s0 to a whole number): the counter's whole frames, its fraction carried at its key (the
   address the patches named: the counter's own, or its object) */
int hos_site_whole_pickup(Ctx *A64_NOESC c) {
    c->x[12] = hos_frame_whole(c->x[19] + 864, (uint32_t)c->vreg[0][0]);
    return 0;
}

/* (the water's substeps carried: w9 the whole substeps, w8, which the game multiplies w9 by, 1) */
int hos_site_whole_water(Ctx *A64_NOESC c) {
    c->x[9] = hos_frame_whole_times(c->x[21], (uint32_t)c->vreg[0][0], (uint32_t)c->x[8]);
    c->x[8] = 1;
    return 0;
}

int hos_site_whole_time_of_day(Ctx *A64_NOESC c) {
    c->x[8] = hos_frame_whole(c->x[19] + 60, (uint32_t)c->vreg[0][0]);
    return 0;
}

/* (frame.wholeScarecrowPlay and frame.wholeScarecrowLast: the song's elapsed frames) */
int hos_site_whole_scarecrow(Ctx *A64_NOESC c) {
    c->x[8] = hos_frame_whole(c->x[19] + 0x265fc, (uint32_t)c->vreg[0][0]);
    return 0;
}

int hos_site_whole_scarecrow_record(Ctx *A64_NOESC c) {
    c->x[8] = hos_frame_whole(c->x[9], (uint32_t)c->vreg[0][0]);
    return 0;
}

int hos_site_whole_floor_web(Ctx *A64_NOESC c) {
    c->x[22] = hos_frame_whole(c->x[19] + 2296, (uint32_t)c->vreg[0][0]);
    return 0;
}

int hos_disable_camera_centering(void);   /* (host/mouse_native.c: the Controls row) */

/* camera.followYaw (jump, before the movement path's first instruction, to the other yaw paths' join): with the row on, x25 is
   its parameter base again, as the skipped path leaves it, and the path is skipped */
int hos_site_camera_follow_yaw(Ctx *A64_NOESC c) {
    if (!hos_disable_camera_centering()) return 0;
    c->x[25] = c->x[26];
    return 1;
}

int hos_oot_floor_carry_slow(uint32_t inv_dt_bits, uint64_t slot);   /* (game/oot_high_fps.c) */

/* physics.floorCarry and the two after it (allow, the b.gt to the tenth): asked only when the game's own test holds; the slot is
   the guest address of the pointer to the game's time object */
int hos_site_floor_carry(Ctx *A64_NOESC c) {
    return hos_oot_floor_carry_slow((uint32_t)c->vreg[0][0], OOT_TIME_SLOT);
}

void hos_oot_voice_option(Ctx *A64_NOESC c, A64Fn game);   /* (game/oot_voice.c) */

/* options.voiceWritten (replace bl f_80a9d1c0, x30 set): the host's wrapper, given the game's function */
int hos_site_voice_written(Ctx *A64_NOESC c) {
    hos_oot_voice_option(c, a64_lookup(hos_site_callee));
    return 0;
}

void hos_oot_gpu_alloc(Ctx *A64_NOESC c, A64Fn game);   /* (game/oot_heap_retry.c) */
extern int hos_oot_heap_test_enabled;
int hos_oot_heap_refuse(Ctx *c, uint64_t heap, uint64_t holder, uint64_t lr);
void hos_oot_heap_retry(Ctx *c, uint64_t heap, uint64_t size, uint64_t align, uint64_t lr, uint64_t holder);
void hos_oot_heap_rest_own(Ctx *c, uint64_t holder);
void hos_oot_heap_rest_end(Ctx *c, uint64_t holder);

/* heap.gpuAlloc and heap.gpuAllocOther (replace bl f_80522050, x30 set): the host's wrapper, given the game's function */
int hos_site_gpu_alloc(Ctx *A64_NOESC c) {
    hos_oot_gpu_alloc(c, a64_lookup(hos_site_callee));
    return 0;
}

/* The CPU allocations (replace blr x8, x30 set; x0 the heap, x1 the size, x2 the alignment): the heap's tryAlloc (not
   called while a test switch refuses it: x0 = 0), and a null tried in the heap's parents, which hands the rest of the
   build to the parent that had room through the heaps' holder, read as the place reads it, before the call */
static void heap_alloc(Ctx *A64_NOESC c, uint64_t holder) {
    const uint64_t heap = c->x[0], size = c->x[1], align = c->x[2], lr = c->x[30];
    if (hos_oot_heap_test_enabled && hos_oot_heap_refuse(c, heap, holder, lr)) c->x[0] = 0;
    else a64_icall(c, c->x[8]);
    if (!c->x[0]) hos_oot_heap_retry(c, heap, size, align, lr, holder);
}
/* heap.cpuAlloc and heap.cpuAllocOther, in f_804566e0: the holder is its second argument, kept at sp+56 */
int hos_site_heap_alloc(Ctx *A64_NOESC c) {
    heap_alloc(c, recomp_load64(c, c->x[31] + 56));
    return 0;
}
/* heap.objectAlloc, f_8044f420's 0x238-byte object: the holder is x21 */
int hos_site_heap_alloc_x21(Ctx *A64_NOESC c) {
    heap_alloc(c, c->x[21]);
    return 0;
}
/* heap.objectBuffer and heap.objectBufferOther, f_8044ed60's two buffers: the holder is x22 (its heap just loaded from x22+8) */
int hos_site_heap_alloc_x22(Ctx *A64_NOESC c) {
    heap_alloc(c, c->x[22]);
    return 0;
}
/* heap.blockBuffer and heap.blockObject, f_80443000's buffer and 0x310-byte object: the holder is its fourth argument, kept
   at sp+0 (its heap just loaded from there) */
int hos_site_heap_alloc_sp0(Ctx *A64_NOESC c) {
    heap_alloc(c, recomp_load64(c, c->x[31]));
    return 0;
}
/* heap.restOwn and heap.restEnd (before blr x8, f_804566e0's getFreeSize calls of its heap: x0 the heap, x8 the method,
   both just loaded through the holder at sp+56): a build handed to a parent reads its own heap */
int hos_site_heap_rest_own(Ctx *A64_NOESC c) {
    hos_oot_heap_rest_own(c, recomp_load64(c, c->x[31] + 56));
    return 0;
}
int hos_site_heap_rest_end(Ctx *A64_NOESC c) {
    hos_oot_heap_rest_end(c, recomp_load64(c, c->x[31] + 56));
    return 0;
}

uint64_t hos_oot_song_cue(uint32_t site, uint64_t game);   /* (game/oot_high_fps.c) */

/* song.noteCue (before the instruction after f_832b2af0's call of f_832b7b50): the flag through the host, as site 0 */
int hos_site_song_cue_played(Ctx *A64_NOESC c) {
    c->x[0] = hos_oot_song_cue(0, c->x[0]);
    return 0;
}

/* song.noteCueOther (the same after f_832b2dc0's call): site 1 */
int hos_site_song_cue_other(Ctx *A64_NOESC c) {
    c->x[0] = hos_oot_song_cue(1, c->x[0]);
    return 0;
}

extern volatile int hos_oc_wait, hos_oc_last_kind;   /* (host/nn_hid.c: the ocarina's note controls) */
extern volatile unsigned hos_oc_beat;
/* (the input kinds the note buttons are for: 1 and 3 the field ocarina, 2 a song lesson, 7 the Lost Woods skull kids' songs, 10
   the scarecrow's recording, 11 its recall; 13, a dialogue's, stays the game's) */
#define HOS_OC_BUTTONS(k) ((k) == 1 || (k) == 2 || (k) == 3 || (k) == 7 || (k) == 10 || (k) == 11)

/* ocarina.waitStart (after ldr w1, [x19, #0x20]): the wait began, for the note buttons or not by its kind */
int hos_site_oc_wait_start(Ctx *A64_NOESC c) {
    hos_oc_last_kind = (int)c->x[1], hos_oc_wait = HOS_OC_BUTTONS(c->x[1]) ? 1 : 2, hos_oc_beat++;
    return 0;
}

/* ocarina.waitTick (before tbz w8, #1 past the tick): not running, no wait; running, re-armed and the beat stamped */
int hos_site_oc_wait_tick(Ctx *A64_NOESC c) {
    if (!((c->x[8] >> 1) & 1)) hos_oc_wait = 0;
    else hos_oc_wait = HOS_OC_BUTTONS(hos_oc_last_kind) ? 1 : 2, hos_oc_beat++;
    return 0;
}

/* ocarina.waitMode (before the mode check's b.ne away): the mode refused (z clear), no wait */
int hos_site_oc_wait_mode(Ctx *A64_NOESC c) {
    if (!c->z) hos_oc_wait = 0;
    return 0;
}

/* ocarina.waitContext and ocarina.waitOwner (before the cbnz / cbz on the answer): none, no wait */
int hos_site_oc_wait_none(Ctx *A64_NOESC c) {
    if (!c->x[0]) hos_oc_wait = 0;
    return 0;
}

/* ocarina.waitDone (after the done store) and ocarina.waitReset (after the input reset's call): no wait */
int hos_site_oc_wait_end(Ctx *A64_NOESC c) {
    (void)c;
    hos_oc_wait = 0;
    return 0;
}

uint32_t hos_loop_step(uint64_t at, uint32_t step_bits, uint32_t start_bits, uint32_t end_bits);   /* (host/nn_vi.c) */
uint32_t hos_loop_pose(uint32_t time_bits, uint32_t start_bits, uint32_t end_bits);

uint64_t recomp_fpe_fma32(Ctx *A64_NOESC c, uint64_t a, uint64_t b, uint64_t z);   /* (the runtime's exact float helper) */

/* anim.loopStep and anim.loopStepOther (replace fmul s<d>, s8, s<m>): s<d> = the host's step for the entry at x20 times s<m>,
   its zero's sign from the operands', through the exact helper the lift's own falls back to; s8 keeps the game's step */
static void loop_step_mul(Ctx *A64_NOESC c, int d, int m) {
    uint64_t a = 0, b = 0;
    memcpy(&a, c->vreg[8], 4);
    a = hos_loop_step(c->x[20], (uint32_t)a, (uint32_t)recomp_load32(c, c->x[20] + 16), (uint32_t)recomp_load32(c, c->x[20] + 20));
    memcpy(&b, c->vreg[m], 4);
    const uint64_t v = recomp_fpe_fma32(c, a, b, (a ^ b) & 0x80000000ULL);
    c->vreg[d][0] = v, c->vreg[d][1] = 0;
}

/* anim.loopStep (replace fmul s2, s8, s1) */
int hos_site_loop_step(Ctx *A64_NOESC c) {
    loop_step_mul(c, 2, 1);
    return 0;
}

/* anim.loopStepOther (replace fmul s1, s8, s0) */
int hos_site_loop_step_other(Ctx *A64_NOESC c) {
    loop_step_mul(c, 1, 0);
    return 0;
}

/* anim.loopPose (replace str s0, [x22]): the host's time for drawing stored (the range at +4, +8 of x22); s0 kept */
int hos_site_loop_pose(Ctx *A64_NOESC c) {
    uint64_t v = 0;
    memcpy(&v, &c->vreg[0][0], 4);
    v = hos_loop_pose((uint32_t)v, (uint32_t)recomp_load32(c, c->x[22] + 4), (uint32_t)recomp_load32(c, c->x[22] + 8));
    recomp_store32(c, c->x[22] + 0ULL, v);
    return 0;
}

uint64_t recomp_fpe_div32(Ctx *A64_NOESC c, uint64_t a, uint64_t b);   /* (the runtime's exact float helpers: a64_suyu) */
uint64_t recomp_fpe_fma32(Ctx *A64_NOESC c, uint64_t a, uint64_t b, uint64_t z);

/* steer.rate* (replace fmul s<d>, s0, s<m>): s<d> = s0 / s<m> while 0 < s8 (the step) < 1, its bits tested as the patch did
   (no host float compare, no guest flag changed); else the game's multiply, s0 * s<m> (its zero's sign from the operands'),
   both through the exact helpers the lift's own fall back to, so the value and FPSR are the Arm ones */
static void turn_bound(Ctx *A64_NOESC c, int d, int m) {
    uint64_t a = 0, b = 0;
    memcpy(&a, c->vreg[0], 4);
    memcpy(&b, c->vreg[m], 4);
    const uint64_t z = (a ^ b) & 0x80000000ULL;
    const uint32_t step = (uint32_t)c->vreg[8][0];
    const uint64_t v = step > 0 && step < 0x3f800000u ? recomp_fpe_div32(c, a, b) : recomp_fpe_fma32(c, a, b, z);
    c->vreg[d][0] = v, c->vreg[d][1] = 0;
}

/* steer.rateLower (replace fmul s1, s0, s1) */
int hos_site_turn_rate_lower(Ctx *A64_NOESC c) {
    turn_bound(c, 1, 1);
    return 0;
}

/* steer.rateUpper (replace fmul s0, s0, s8) */
int hos_site_turn_rate_upper(Ctx *A64_NOESC c) {
    turn_bound(c, 0, 8);
    return 0;
}

int hos_oot_barrier_on(void);   /* (game/oot_barrier.c) */
int hos_oot_barrier_wait(uint64_t p, uint32_t off1, uint32_t seen1, uint32_t off2, uint32_t seen2, uint32_t full);
void hos_oot_barrier_wake(uint64_t p);

/* barrier.sleep* (replace bl SleepThread's stub, x30 set): the wait, its words read only when the switch is on, as the
   game's sleep when it is off or the wait answers 0 */
int hos_site_barrier_sleep_claim(Ctx *A64_NOESC c) {   /* (barrier.sleepClaim) */
    const uint64_t sleep = hos_site_callee;   /* (SleepThread, as the lift calls it from the jobs' sleeps) */
    if (!hos_oot_barrier_on() || !hos_oot_barrier_wait(c->x[24], 500, (uint32_t)recomp_load32(c, c->x[24] + 500), 508, (uint32_t)recomp_load32(c, c->x[24] + 508), (uint32_t)c->x[22]))
        a64_lookup(sleep)(c);
    return 0;
}

int hos_site_barrier_sleep_gather(Ctx *A64_NOESC c) {   /* (barrier.sleepGather) */
    const uint64_t sleep = hos_site_callee;   /* (SleepThread, as the lift calls it from the jobs' sleeps) */
    if (!hos_oot_barrier_on() || !hos_oot_barrier_wait(c->x[24], 512, (uint32_t)c->x[25], 512, (uint32_t)c->x[25], (uint32_t)c->x[22]))
        a64_lookup(sleep)(c);
    return 0;
}

int hos_site_barrier_sleep_done(Ctx *A64_NOESC c) {   /* (barrier.sleepDone) */
    const uint64_t sleep = hos_site_callee;   /* (SleepThread, as the lift calls it from the jobs' sleeps) */
    if (!hos_oot_barrier_on() || !hos_oot_barrier_wait(c->x[24], 520, (uint32_t)c->x[19], 520, (uint32_t)c->x[19], (uint32_t)recomp_load32(c, c->x[29] - 100)))
        a64_lookup(sleep)(c);
    return 0;
}

int hos_site_barrier_sleep_other(Ctx *A64_NOESC c) {   /* (barrier.sleepOther) */
    const uint64_t sleep = hos_site_callee;   /* (SleepThread, as the lift calls it from the jobs' sleeps) */
    if (!hos_oot_barrier_on() || !hos_oot_barrier_wait(c->x[19], 528, (uint32_t)c->x[25], 536, (uint32_t)c->x[28], (uint32_t)recomp_load32(c, c->x[31] + 16)))
        a64_lookup(sleep)(c);
    return 0;
}

int hos_site_barrier_sleep_third(Ctx *A64_NOESC c) {   /* (barrier.sleepThird) */
    const uint64_t sleep = hos_site_callee;   /* (SleepThread, as the lift calls it from the jobs' sleeps) */
    if (!hos_oot_barrier_on() || !hos_oot_barrier_wait(c->x[26], 544, (uint32_t)c->x[21], 544, (uint32_t)c->x[21], ~(uint32_t)c->x[28]))
        a64_lookup(sleep)(c);
    return 0;
}

/* barrier.or* (after the ldset into a polled word): the waiters on the job block woken, P the OR's base register */
int hos_site_barrier_wake_x24(Ctx *A64_NOESC c) {
    if (hos_oot_barrier_on()) hos_oot_barrier_wake(c->x[24]);
    return 0;
}

int hos_site_barrier_wake_x19(Ctx *A64_NOESC c) {
    if (hos_oot_barrier_on()) hos_oot_barrier_wake(c->x[19]);
    return 0;
}

int hos_site_barrier_wake_x26(Ctx *A64_NOESC c) {
    if (hos_oot_barrier_on()) hos_oot_barrier_wake(c->x[26]);
    return 0;
}

extern volatile int hos_fixed_resolution, hos_dynres_runs;   /* (host/nn_vi.c) */
extern volatile uint32_t hos_fixed_scale_bits;

/* dynres.fixedPath (branch, before tbz w8, #0 to the fixed path): taken while the host holds the fixed path */
int hos_site_dynres_fixed_path(Ctx *A64_NOESC c) {
    (void)c;
    return hos_fixed_resolution;
}

/* dynres.dynamicFrame (before the instruction after the test: the dynamic path's frames) */
int hos_site_dynres_dynamic_frame(Ctx *A64_NOESC c) {
    (void)c;
    hos_dynres_runs++;
    return 0;
}

/* dynres.fixedScale (replace mov w9, #0x3f800000): w9 = the fixed path's scale, 1.0 as shipped */
int hos_site_dynres_fixed_scale(Ctx *A64_NOESC c) {
    c->x[9] = hos_fixed_scale_bits;
    c->x[9] &= 0xFFFFFFFFULL;
    return 0;
}

extern volatile uint64_t hos_res_module;   /* (host/nn_vi.c: f_831f3200's object, whose +0xe5c is the 3D's scale) */

/* render.fixedScaleApply (before the update of +0xe2c): on the fixed path, a recompute asked for (w8 bit 3) while the 3D's
   scale (+0xe5c) is not the chosen one (scale * +0xe58 / +0xe50): a recompute every frame remade the targets */
int hos_site_fixed_scale_apply(Ctx *A64_NOESC c) {
    if (hos_fixed_resolution) {
        union { uint32_t u; float f; } now = {(uint32_t)recomp_load32(c, c->x[20] + 3676)}, want = {hos_fixed_scale_bits},
            e50 = {(uint32_t)recomp_load32(c, c->x[20] + 3664)}, e58 = {(uint32_t)recomp_load32(c, c->x[20] + 3672)};
        const float d = now.f - (e50.f != 0 ? want.f * e58.f / e50.f : want.f);
        if (d > 1e-4f || d < -1e-4f) c->x[8] |= 8;
    }
    return 0;
}

/* render.fixedScaleFlags (after ldr x8, [x20, #0xe20]): the object for the host; on the fixed path the flags' bits 3/4 (a
   hardcoded 0.75 / 0.5) not looked at */
int hos_site_fixed_scale_flags(Ctx *A64_NOESC c) {
    hos_res_module = c->x[20];
    if (hos_fixed_resolution) c->x[8] &= ~0x18ULL;
    return 0;
}

uint64_t hos_oot_ocb_pick(Ctx *c, uint64_t mgr, uint64_t chosen);   /* (game/oot_ocb_hold.c: HOS_OCB_HOLD_MS) */

/* preload.choice (before the update's test of its choice, tbnz w21, #31): x21 = the unit the update goes on with, the
   held neighbour while another choice has not lasted the hold (the game's own with HOS_OCB_HOLD_MS=0); x19 the manager */
int hos_site_preload_choice(Ctx *A64_NOESC c) {
    c->x[21] = hos_oot_ocb_pick(c, c->x[19], c->x[21]);
    return 0;
}

void hos_sprint_link(Ctx *c);     /* (game/oot_sprint.c: Sprint) */
void hos_sprint_stance(Ctx *c);

/* sprint.link (after Link's movement update loads his input flags into x10; x19 Link): the dash input ORed in while Sprint
   runs */
int hos_site_sprint_link(Ctx *A64_NOESC c) {
    hos_sprint_link(c);
    return 0;
}

/* sprint.stance (after the stance and camera update loads the flags' low byte into x8; x19 Link) */
int hos_site_sprint_stance(Ctx *A64_NOESC c) {
    hos_sprint_stance(c);
    return 0;
}

void hos_oot_jump_speed(uint64_t vec);   /* (game/oot_high_fps.c: a jump's takeoff above 60 fps) */
void hos_oot_takeoff_velocity(uint64_t object, uint64_t velocity, uint64_t slot);

/* jump.takeoffSpeed (after f_80b97d10's last store of the takeoff speed to the vector at x19): the speed kept */
int hos_site_jump_takeoff_speed(Ctx *A64_NOESC c) {
    hos_oot_jump_speed(c->x[19]);
    return 0;
}

/* jump.takeoffVelocity (before f_804f5030's set-velocity call: x19 the body's object, x2 its linear velocity) */
int hos_site_jump_takeoff_velocity(Ctx *A64_NOESC c) {
    hos_oot_takeoff_velocity(c->x[19], c->x[2], OOT_TIME_SLOT);
    return 0;
}

void hos_oot_fov(Ctx *c, uint64_t camera);           /* (game/crates/oot/src/fov.rs: the gameplay field of view, in Rust) */
void hos_oot_shadow(Ctx *c, uint64_t shadow);        /* (game/oot_shadow.c: the sun's shadow resolution) */
void hos_shadow_fit_basis(Ctx *c, uint64_t cascade); /* (game/oot_shadow_fit.c: steady sun shadows) */
int hos_shadow_fit_box(Ctx *c, uint64_t cascade, float b[4]);

/* camera.lens (after the camera update loads its vertical angle into s0; x19 the camera) */
int hos_site_camera_lens(Ctx *A64_NOESC c) {
    hos_oot_fov(c, c->x[19]);
    return 0;
}

/* shadow.cascadeSize (after the cascade size's rounding to a multiple of 8, before its store; x19 the shadow object) */
int hos_site_shadow_cascade_size(Ctx *A64_NOESC c) {
    hos_oot_shadow(c, c->x[19]);
    return 0;
}

/* shadow.fitBasis (after the basis builder's call; x19 the cascade) */
int hos_site_shadow_fit_basis(Ctx *A64_NOESC c) {
    hos_shadow_fit_basis(c, c->x[19]);
    return 0;
}

/* shadow.fitBox (where the box fit's modes meet; x20 the cascade): the slice's bounds, b[] = min x (s5), max x (s4), min y
   (s2), max y (s3), handed to the host, and written back as whole registers (the rest of each zeroed) when it changes them */
int hos_site_shadow_fit_box(Ctx *A64_NOESC c) {
    static const int reg[4] = {5, 4, 2, 3};
    float b[4];
    for (int i = 0; i < 4; i++) memcpy(&b[i], c->vreg[reg[i]], 4);
    if (hos_shadow_fit_box(c, c->x[20], b))
        for (int i = 0; i < 4; i++) {
            uint32_t u;
            memcpy(&u, &b[i], 4);
            a64_vset64(c->vreg[reg[i]], u);
        }
    return 0;
}

int hos_dlss_jitter_clip(uint64_t projection, float out[2]);   /* (host/rx_backend.c: the upscalers' camera jitter) */

/* The jitter added to the third lane (bytes 8 to 11) of the registers holding the projection's rows 0 (x) and 1 (y), -1 for a
   row not loaded at the site; the view is x28, its projection at +0x1a8 */
static void jitter_rows(Ctx *A64_NOESC c, int xreg, int yreg) {
    float j[2];
    if (!hos_dlss_jitter_clip(c->x[28] + 0x1a8, j)) return;
    const int rows[2] = {xreg, yreg};
    for (int axis = 0; axis < 2; axis++) {
        if (rows[axis] < 0) continue;
        float o;
        memcpy(&o, (uint8_t *)c->vreg[rows[axis]] + 8, 4);
        o += j[axis];
        memcpy((uint8_t *)c->vreg[rows[axis]] + 8, &o, 4);
    }
}

/* jitter.viewRows (after ldp q21, q22: rows 0 and 1 are in q19 and q20) */
int hos_site_jitter_view_rows(Ctx *A64_NOESC c) {
    jitter_rows(c, 19, 20);
    return 0;
}

/* jitter.viewRowX (after ldr q26: row 0) */
int hos_site_jitter_view_row_x(Ctx *A64_NOESC c) {
    jitter_rows(c, 26, -1);
    return 0;
}

/* jitter.viewRowY (after ldr q6: row 1) */
int hos_site_jitter_view_row_y(Ctx *A64_NOESC c) {
    jitter_rows(c, -1, 6);
    return 0;
}
