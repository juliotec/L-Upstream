// NVIDIA DLSS Super Resolution, one of the temporal upscaler's engines (vk_temporal.h; vk_dlss.cpp). Built in only with
// NVIDIA's NGX SDK (LONGSHOT_NGX, scripts/fetch.py dlss: Linux and Windows); without it these say DLSS is not there and the
// game's network runs.
#pragma once

#include <vector>
#include "dlss_nr_support.h"
#include "dlss_nr_settings.h"
#include "nr_metrics.h"

#include "vk_internal.h"

namespace mkwvk {

struct TemporalEngine;
struct TemporalFrame;
struct TemporalImage;

/// Whether this build has DLSS in it (LONGSHOT_NGX)
bool dlss_built();

// ---- DLSS 5 (NVIDIA's neural rendering, NGX feature 18): after the temporal upscaler, before the HUD ----
/// The model's working size this frame, a percent of the picture (the DLSS 5 model resolution row's)
int dlss_nr_scale();
void dlss_nr_idle();
bool dlss_nr_pending();   // (a check asked for (dlss_nr_retry) waits for the recording thread)
/// The model over the upscaler's picture (vk_temporal.cpp draw_engine): true with result the picture to show instead of
/// f.output; false, nothing done (off, not available, or failed: the upscaler's picture is shown)
bool dlss_nr_chain(VkCommandBuffer cmd, TemporalFrame& f, bool native_hdr, TemporalImage& result);
/// The menu's DLSS 5 rows show (gpu/vk/menu/dlss5.cpp): NVIDIA's model, nvngx_dlssnr.dll, is in one of the folders NGX
/// searches for DLSS's runtime (runtime_dirs: RX_VK_DLSS_DIR, the game's ngx folder, LOCALAPPDATA's Longshot/ngx), or
/// (Windows) the known community FP16 one in such a folder's fp16 folder (dlss_nr_model.h), on an NVIDIA GPU, in a build
/// with DLSS. Linux: only with RX_VK_DLSS_NR_LINUX=1 (native NGX finds its own runtime). Looked for once a run: a DLL put in
/// while the game runs shows the rows at the next start
bool dlss_nr_offered();
/// The DLSS 5 model row shows (gpu/vk/menu/dlss5.cpp): an RTX 50 with both files, NVIDIA's and the known FP16 one (Windows)
bool dlss_nr_model_offered();
enum DlssNrStatus { NR_WAITING, NR_ACTIVE, NR_PENDING, NR_FAILED };
DlssNrStatus dlss_nr_status();   // (the last evaluation's, for the menu: never inferred from the On row)
dlss_nr::Issue dlss_nr_check(bool refresh = false);   // (the last check's; refresh asks the recording thread to check again)
dlss_nr::Issue dlss_nr_issue();   // (why DLSS 5 does not run now, whether or not it is On)
void dlss_nr_retry();             // (checked again: the menu's DLSS 5 row turned On)
/// What the menu's DLSS 5 rows hold, for the renderer (the recording thread takes a copy each frame)
struct DlssNrControls {
    bool requested = false;
    dlss_nr::Settings model;   // (latched when the model's feature is made: a change makes it again, once it settles)
    int detail = 100, color = 100;   // (the composite's, at once: hundredths of the way to the model's picture, of its colour)
    int highlight = 20;              // (the most the model may brighten or darken a pixel, tenths of a ratio)
    int debug = 0;                   // (RX_VK_DLSS_NR_DEBUG's view for this run, never saved)
    int scale = 100;                 // (the model's working size, a percent of the picture)
    int model_file = 0;              // (the DLSS 5 model row as the run started, dlss_nr::ModelChoice: which file runs)
};
void dlss_nr_publish(const DlssNrControls& controls);
/// Lock-free telemetry for the FPS counter's Dev level; owned_bytes leaves out what NVIDIA's runtime allocates itself
dlss_nr::MetricsView dlss_nr_metrics();
/// Why DLSS cannot be chosen here (the Insert menu's Upscaler row says it), DLSS_OK when it can or is not known yet
enum DlssWhy { DLSS_OK, DLSS_NOT_BUILT, DLSS_ENV_OFF, DLSS_NEEDS_RTX, DLSS_NEEDS_DRIVER, DLSS_NO_RUNTIME,
               DLSS_RUNTIME_NOT_ASCII, DLSS_WHY_COUNT };
DlssWhy dlss_why();
/// Asks NGX once whether DLSS runs on this device, so dlss_why knows before the first upscale (the menu, when it opens).
/// Nothing off an NVIDIA device with NGX's extensions, or in a build without DLSS
void dlss_probe();
/// A feature that was not made is asked for again at the next upscale (vk_dlss.cpp prepare's refusal): the player changed
/// a row the feature is made from (vk_temporal.cpp temporal_reset_failure, with the chain's lock held, as prepare has it).
/// A refusal for want of video memory is also retried after a bounded interval without this.
void dlss_retry();
/// (longshot, RX_VK_DLSS_PREWARM) Whether NGX has started, without waiting for it: 1 started, 0 it will not start here, -1
/// not asked yet or being asked now (dlss_probe's thread holds its lock: the replay does not wait for its ~450 ms)
int dlss_ngx_ready();
/// The instance's extensions DLSS needs, added to `want` when the loader has them (all: what it has)
void dlss_instance_extensions(std::vector<const char*>& want, const std::vector<VkExtensionProperties>& all);
/// The device's extensions DLSS needs, added to `want` when the device has them
void dlss_device_extensions(std::vector<const char*>& want, const std::vector<VkExtensionProperties>& all);
/// DLSS as the temporal upscaler's engine (DLSS and DLAA on the Upscaler row); nullptr in a build without it
const TemporalEngine* dlss_engine();

// NVIDIA DLSS Frame Generation through NGX (vk_dlss.cpp): the renderer's frame generation (vk_framegen.cpp) shows and paces
// the frames DLSS-G makes from the temporal chain's depth, motion and camera. Asked for with RX_VK_DLSS_FG=1 for a run.
struct TemporalFrame;
/// Whether it is asked for now: the Insert menu's Frame generation by row (DLSS FG), RX_VK_DLSS_FG=0 / 1 over it (and the
/// build has it)
bool dlss_fg_wanted();
/// (longshot) Why the last set it was asked for went to the renderer's own frame generation, for the Insert menu's Frame
/// generation by row (its texts are the menu's, to be translated): FG_SKIP_NONE when DLSS-G made it or nothing is known.
/// most: its runtime's most made frames a set (FG_SKIP_TOO_MANY)
enum FgSkip {
    FG_SKIP_NONE,
    FG_SKIP_OFF,        // (RX_VK_DLSS_FG=0 for the run)
    FG_SKIP_DEVICE,     // (DLSS does not run on this GPU)
    FG_SKIP_START,      // (NGX did not start it: no runtime, an older GPU)
    FG_SKIP_TOO_MANY,   // (more made frames than its runtime makes)
    FG_SKIP_INPUTS,     // (no picture from a temporal Upscaler this frame)
    FG_SKIP_REFUSED,    // (NGX refused the set)
    FG_SKIP_NOT_BUILT,  // (a build without NVIDIA's SDK)
    FG_SKIP_COUNT
};
int dlss_fg_skip(unsigned* most = nullptr);
/// (longshot) Why DLSS-G cannot run in this process at all, for the Frame generation by row, which offers DLSS FG only where
/// it can: FG_SKIP_NONE while it can as far as is known (NGX not asked yet, or its yes), else for the rest of the run
/// FG_SKIP_NOT_BUILT, FG_SKIP_DEVICE (DLSS does not run here, dlss_why says why: RX_VK_DLSS=0 among them), FG_SKIP_OFF
/// (RX_VK_DLSS_FG=0), FG_SKIP_START (NGX's no) or FG_SKIP_REFUSED (its feature given up). Read without a lock, by the menu
int dlss_fg_unavailable();
/// (longshot) RX_VK_DLSS_FG=1 for the run: DLSS-G asked for whatever the Frame generation by row says (the row shows it so)
bool dlss_fg_forced();
/// (longshot) NGX asked once a run whether DLSS-G runs here (its runtime, the GPU and the driver), before any present needs
/// it, so the Frame generation by row knows its choices: at the start-up screen on a thread of its own (vk_menu.cpp
/// probe_upscalers_soon), and when the menu opens on the present thread (menu_tick, present_image_mu held). The answer is
/// logged
void dlss_fg_probe();
/// (longshot) Whether the set dlss_fg_frames did not make is the game's frame alone (vk_framegen.cpp framegen_record): DLSS FG
/// asked for, on a temporal Upscaler (vk_temporal.h temporal_wanted), and no picture from it for this set, as in the game's
/// own menus, movies and loading screens. Else (another Upscaler, a refusal, a set of more frames than its runtime makes
/// before NGX's answer) the built-in one makes them, as before. Right after dlss_fg_frames, on its thread
bool dlss_fg_holds();
/// Whether it makes the sets of `frames` made frames (the multiplier less 1) as far as is known: asked for, on a device DLSS
/// runs on, NGX's answer not a no, its feature not given up and its runtime making that many (an RTX 40's makes 1 at the
/// most; the multiplier as it runs stops there, dlss_fg_most). The swapchain's present mode follows it: vk_main.cpp
/// swapchain_dlssg
bool dlss_fg_runs(int frames);
/// (longshot) The most made frames its runtime makes a set (NGX's MultiFrameCountMax: 5 on an RTX 50's runtime, 1 on an RTX
/// 40's) while DLSS-G is to make the frames: asked for, on a device DLSS runs on, a temporal Upscaler chosen (its inputs),
/// NGX's answer a yes and its feature not given up. 0 otherwise, and before NGX has answered. Frame generation's fixed
/// multiplier stops at it + 1 (vk_framegen.h framegen_now): past it NGX was not asked for the sets (FG_SKIP_TOO_MANY) and the
/// renderer's own made every frame
int dlss_fg_most();
/// Frame generation's device extensions, asked of NGX, added to `want` when the device has them
void dlss_fg_device_extensions(std::vector<const char*>& want, const std::vector<VkExtensionProperties>& all);
/// The chain's frame (vk_temporal.cpp, after its engine's evaluation): its depth and motion copied, its camera kept, for the
/// next present. projection and view_projection: the scene's, jittered by clip_jitter; motion: the chain had the frame before's
void dlss_fg_inputs(VkCommandBuffer cmd, const TemporalFrame& f, const float projection[16], const float view_projection[16],
                    const float clip_jitter[2], bool motion);
/// At the present (vk_framegen.cpp framegen_record): `count` frames between the last picture and this one (color, the
/// window's, w x h RGBA8) into out[0..count). Returns count, or 0 when it did not make them (the renderer's own methods do).
/// blend: there is a picture before this one; sequence: the present's count (one skipped makes a new history)
int dlss_fg_frames(VkCommandBuffer cmd, VkImage color, VkImageView color_view, uint32_t w, uint32_t h, int count,
                   const VkImage* out, const VkImageView* out_views, bool blend, uint64_t sequence);
/// Frame generation at Off or Auto (no fixed multiplier: dlss_fg_frames is not asked): its feature, if made, let go and its
/// memory back, as when the Frame generation by row changes. Each such present (vk_replay.cpp); nothing to do is one load
void dlss_fg_idle();
/// Before the process exits (mkwvk_close_asked): its feature let go with the queue idle
void dlss_fg_close();

} // namespace mkwvk
