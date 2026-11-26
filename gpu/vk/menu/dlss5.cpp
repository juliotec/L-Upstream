// The Insert menu's DLSS 5 rows: NVIDIA's neural rendering model over the upscaler's picture, before the HUD (vk_dlss.cpp
// dlss_nr_chain). On the Graphics tab right under the Upscaler's rows (after Sharpness): DLSS 5, its preset and a row that
// opens the DLSS 5 settings page, which holds the tuning rows (vk_menu.cpp TAB_DLSS5). Shown only where it can run: NVIDIA's
// nvngx_dlssnr.dll found in one of the folders NGX searches (the game's ngx folder, LOCALAPPDATA's Longshot/ngx or
// RX_VK_DLSS_DIR; the player puts it there, Longshot neither ships nor downloads it), an NVIDIA GPU, a build with DLSS, and
// a temporal Upscaler (DLSS, DLAA or FSR). On Windows the community FP16 conversion of that file counts too, in such
// a folder's fp16 folder (dlss_nr_model.h: it runs DLSS 5 on RTX 20, 30 and 40); on an RTX 50 with both files the
// DLSS 5 model row chooses which one runs, after a restart. With none of that there is no row and no line (no heading
// of their own either: the menu draws a section's title over rows all hidden). The rows are the Bloodborne PC port's
// DLSS 5 rows; the Default preset
// holds them as tuned in the game (menu/dlss5.h), and a row moved by hand makes the preset row say Custom. The rows past the first show while DLSS 5
// is On (the preset, the page's row and the page's rows). The parts that ask the renderer are fenced from a layout-only
// build (MENU_LAYOUT_ONLY, MENU_HOOK).
#include "menu_module.h"

#include "menu/dlss5.h"
#include "upscaler_rows.h"
#include "vk_menu_hit.h"

#ifndef MENU_LAYOUT_ONLY
#include "dlss_nr_settings.h"
#include "menu/output.h"   // (game_size: the picture's size)
#include "vk_dlss.h"       // (dlss_nr_publish, dlss_nr_status, dlss_nr_issue)
#endif

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstdlib>
#include <string>

namespace mkwvk {

#ifndef MENU_LAYOUT_ONLY
void dlss5_publish() {
    // (RX_VK_DLSS_NR_DEBUG: a view of the model's inputs or output for this run, never saved)
    static const int debug = [] {
        const char* e = getenv("RX_VK_DLSS_NR_DEBUG");
        return e && *e ? std::clamp(atoi(e), 0, 5) : 0;
    }();
    DlssNrControls c;
    c.requested = opts.dlss5 != 0;
    c.model.intensity = opts.dlss5_intensity, c.model.structure = opts.dlss5_local_structure;
    c.model.tone = opts.dlss5_local_tone, c.model.skin = opts.dlss5_skin;
    c.model.style = opts.dlss5_style, c.model.mask = opts.dlss5_auto_mask;
    c.model.preset = 2;   // (the model's fastest, as the Bloodborne port always asks for it)
    c.model.ui = 1;
    c.model.strengths = opts.dlss5_strengths == 2;   // (Applied; As in OptiScaler is the model's own, as Model defaults)
    c.detail = opts.dlss5_detail, c.color = opts.dlss5_colour, c.highlight = opts.dlss5_highlight;
    c.debug = debug;
    c.scale = opts.dlss5_resolution;
    // (the DLSS 5 model row as the run started: the first publish is menu_init's, and the row applies after a restart)
    static const int model_at_start = opts.dlss5_model;
    c.model_file = model_at_start;
    dlss_nr_publish(c);
}
#endif

int dlss5_model_env() {
    const char* e = getenv("RX_VK_DLSS_NR_MODEL");
    if (!e || !*e) return -1;
    const std::string v = [&] {
        std::string s(e);
        for (char& ch : s) ch = (char)std::tolower((unsigned char)ch);
        return s;
    }();
    return v == "auto" || v == "0" ? 0 : v == "nvidia" || v == "1" ? 1 : v == "fp16" || v == "2" ? 2 : -1;
}

namespace {
// (the renderer told at once: each row's change)
void publish() {
#ifndef MENU_LAYOUT_ONLY
    dlss5_publish();
#endif
}
/// The rows show: DLSS 5 can run here (menu_module.h dlss5_offered: its DLL, an NVIDIA GPU, a build with DLSS) with a
/// temporal Upscaler; the rows past the first, while DLSS 5 is On
bool offered() { return dlss5_offered() && upscaler_rows::supports_nr(upscaler()); }
bool tuning() { return offered() && opts.dlss5 != 0; }
/// The DLSS 5 model row shows while DLSS 5 is On on an RTX 50 with both files, NVIDIA's and the FP16 one (menu_module.h
/// dlss5_model_offered): the only case with a choice
bool model_shown() { return tuning() && dlss5_model_offered(); }

int preset_ui = 0;   // (the DLSS 5 preset row: unsaved, synced from the rows it sets)
bool is_default() {
    const Dlss5Preset& p = kDlss5Default;
    return opts.dlss5_resolution == p.resolution && opts.dlss5_detail == p.detail && opts.dlss5_colour == p.colour &&
           opts.dlss5_style == p.style && opts.dlss5_strengths == p.strengths && opts.dlss5_intensity == p.intensity &&
           opts.dlss5_local_structure == p.local_structure && opts.dlss5_local_tone == p.local_tone &&
           opts.dlss5_skin == p.skin && opts.dlss5_auto_mask == p.auto_mask && opts.dlss5_highlight == p.highlight;
}
void apply_default() {
    const Dlss5Preset& p = kDlss5Default;
    opts.dlss5_resolution = p.resolution, opts.dlss5_detail = p.detail, opts.dlss5_colour = p.colour;
    opts.dlss5_style = p.style, opts.dlss5_strengths = p.strengths, opts.dlss5_intensity = p.intensity;
    opts.dlss5_local_structure = p.local_structure, opts.dlss5_local_tone = p.local_tone, opts.dlss5_skin = p.skin;
    opts.dlss5_auto_mask = p.auto_mask, opts.dlss5_highlight = p.highlight;
}

/// A value of 0 to 2 in hundredths, as the Bloodborne port shows it ("1.54")
std::string hundredths(int v) {
    char b[16];
    snprintf(b, sizeof b, "%d.%02d", v / 100, v % 100);
    return b;
}
std::string skin_text(int v) { return v < 0 ? std::string("Follows local structure") : hundredths(v); }
/// Skin structure's slider: Follows local structure (-100) at its left end, then 0 to 2. The arrows step over the gap
int skin_step(int v, int dir) { return v < 0 ? (dir > 0 ? 0 : -100) : v + dir * 2 < 0 ? -100 : std::min(v + dir * 2, 200); }
int skin_slider_value(int permille) {
    const int v = menu_hit::slider_value(permille, -100, 200, 2);
    return v < 0 ? (v < -50 ? -100 : 0) : v;
}
std::string guard_text(int v) {
    char b[16];
    snprintf(b, sizeof b, "%d.%dx", v / 10, v % 10);
    return b;
}
/// The four strengths reach the model only with Model strengths Applied (vk_dlss.cpp neural_render)
const char* strengths_locked() {
    return opts.dlss5_strengths == 2 ? nullptr : "Sent to the model only with Model strengths at Applied.";
}

#ifndef MENU_LAYOUT_ONLY
std::string tx_resolution(int v) {
    uint32_t w, h;
    game_size(1.0f, w, h);   // (the picture the model works over, after the upscaler)
    char b[48];
    snprintf(b, sizeof b, "%d%% (%ux%u)", v, dlss_nr::extent(w, v), dlss_nr::extent(h, v));
    return b;
}
/// DLSS 5's line: what it does now (the renderer's last evaluation), or why it does not run. Every one: each state, then
/// each reason
const char* hn_dlss5(int v, int variant) {
    static const char* const state[] = {
        "Waiting for the game's 3D picture.",
        "Running: the model works over the upscaler's picture, before the HUD.",
        "Making the model again with the new settings.",
        "Off after a failure (see the log). Turn DLSS 5 Off and On to try again.",
    };
    if (variant < 0) {
        if (!v) return nullptr;
        const dlss_nr::Issue issue = dlss_nr_issue();
        if (issue != dlss_nr::Issue::none) return dlss_nr::message(issue);
        return state[std::clamp((int)dlss_nr_status(), 0, 3)];
    }
    if (variant < 4) return state[variant];
    const int issue = variant - 4 + 1;   // (Issue::none is no line)
    return issue < dlss_nr::kIssues ? dlss_nr::message((dlss_nr::Issue)issue) : nullptr;
}
#endif
} // namespace

MENU_MODULE(dlss5) {
    const auto changed = [](int) { publish(); };
    menu::tab("graphics")
        .row({.key = "dlss5",
              .label = "DLSS 5",
              .help = "[EXPERIMENTAL] NVIDIA's DLSS 5 model adds detail to faces, skin and surfaces after the "
                      "upscaler, before the HUD. Costs a lot of GPU time. Needs NVIDIA's nvngx_dlssnr.dll in the "
                      "game's ngx folder (RTX 50), or the community FP16 one in its fp16 folder (RTX 20, 30, 40 and "
                      "50): Longshot does not include or download them. Applies at once.",
              .kind = menu::Kind::Toggle,
              .value = &opts.dlss5,
              .lo = 0, .hi = 1,
              .preset_exempt = true,
              .shown = offered,
              .hint = MENU_HOOK(hn_dlss5),
              .on_change = [](int v) {
#ifndef MENU_LAYOUT_ONLY
                  if (v) dlss_nr_retry();   // (On: NGX is asked again, a failure before forgotten)
#endif
                  (void)v;
                  publish();
              }}, menu::after("sharpness"))
        .row({.key = "dlss5_model",
              .label = "DLSS 5 model",
              .help = "[EXPERIMENTAL] Which DLSS 5 file runs: NVIDIA's own (FP8, made for RTX 50 cards) or the "
                      "community FP16 conversion of it. Auto is NVIDIA's. Shown on an RTX 50 card with both files. "
                      "Applies after a restart.",
              .value = &opts.dlss5_model,
              .lo = 0, .hi = 2,
              .choices = {"Auto", "NVIDIA", "FP16"},
              .restart = true,
              .preset_exempt = true,
              .shown = model_shown}, menu::after("dlss5"))
        .row({.key = "dlss5_preset",
              .label = "DLSS 5 preset",
              .help = "[EXPERIMENTAL] Default: our settings for every row of the DLSS 5 settings page, tuned in the "
                      "game. Custom shows once you change one of them; choose Default to put them all back.",
              .value = &preset_ui,
              .lo = 0, .hi = 1,
              .choices = {"Default", "Custom"},
              .preset_exempt = true,
              .shown = tuning,
              .step_fn = [](int, int) { return 0; },   // (Custom is what the rows hold, never chosen: either way is Default)
              .on_change = [](int v) {
                  if (v == 0) apply_default();
                  publish();
              },
              .unsaved = true,
              .sync = [] { return is_default() ? 0 : 1; }}, menu::after("dlss5_model"))
        .row({.key = "@DLSS 5 settings",
              .label = "DLSS 5 settings",
              .help = "[EXPERIMENTAL] Opens the DLSS 5 page: model resolution, detail and colour strength, style, the "
                      "model's strengths, the skin mask and the highlight guard.",
              .kind = menu::Kind::Page,
              .opens = "dlss5_settings",
              .shown = tuning}, menu::after("dlss5_preset"));
    // (the DLSS 5 settings page: the Bloodborne port's tuning rows; vk_menu.cpp TAB_DLSS5, Back to Graphics)
    menu::page("dlss5_settings")   // (the core makes the page and names it: vk_menu.cpp kTabIds, kTabLabels)
        .row({.key = "dlss5_resolution",
              .label = "Model resolution",
              .help = "[EXPERIMENTAL] The size the model works at, as a share of the picture. Lower is much faster: "
                      "the model sees an averaged copy of the picture and only its edit is scaled back up, so the "
                      "picture keeps its own sharpness while the model's fine detail softens. 100% is the whole "
                      "picture. Makes the model again (a moment).",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_resolution,
              .lo = 25, .hi = 100, .step = 5,
              .preset_exempt = true,
              .shown = tuning,
              .text = MENU_HOOK(tx_resolution),
              .on_change = changed,
              .commit_on_release = true}, menu::order(10))
        .row({.key = "dlss5_detail",
              .label = "Detail strength",
              .help = "[EXPERIMENTAL] How far the picture moves toward the model's: 0 is the upscaler's alone, 1 the "
                      "model's picture, above 1 further still. Applies at once.",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_detail,
              .lo = 0, .hi = 200, .step = 2,
              .preset_exempt = true,
              .shown = tuning,
              .text = hundredths,
              .on_change = changed}, menu::order(20))
        .row({.key = "dlss5_colour",
              .label = "Colour strength",
              .help = "[EXPERIMENTAL] 0 keeps the game's own colour and takes only the model's light; 1 takes its "
                      "colour too; above 1 more saturated (it can flicker). Applies at once.",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_colour,
              .lo = 0, .hi = 200, .step = 2,
              .preset_exempt = true,
              .shown = tuning,
              .text = hundredths,
              .on_change = changed}, menu::order(30))
        .row({.key = "dlss5_style",
              .label = "Style",
              .help = "[EXPERIMENTAL] Standard: the strongest, it can look stylised. Natural: gentler, closer to the "
                      "game's skin tones. Cinematic: less shine, a film-like look. Makes the model again (a moment).",
              .value = &opts.dlss5_style,
              .lo = 0, .hi = 2,
              .choices = {"Standard", "Natural", "Cinematic"},
              .preset_exempt = true,
              .shown = tuning,
              .on_change = changed}, menu::order(40))
        .row({.key = "dlss5_strengths",
              .label = "Model strengths",
              .help = "[EXPERIMENTAL] Whether the intensity, local structure, local tone and skin structure below "
                      "reach the model. As in OptiScaler: as the OptiScaler DLSS-NR fork gave them on Vulkan, which "
                      "was not at all, so the model's own. Model defaults: NVIDIA's own values. Applied: the values "
                      "below. Makes the model again.",
              .value = &opts.dlss5_strengths,
              .lo = 0, .hi = 2,
              .choices = {"As in OptiScaler", "Model defaults", "Applied"},
              .preset_exempt = true,
              .shown = tuning,
              .on_change = changed}, menu::order(50))
        .row({.key = "dlss5_intensity",
              .label = "Intensity",
              .help = "[EXPERIMENTAL] The model's own strength (NVIDIA's default 1). Makes the model again.",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_intensity,
              .lo = 0, .hi = 200, .step = 2,
              .preset_exempt = true,
              .shown = tuning,
              .locked = strengths_locked,
              .text = hundredths,
              .on_change = changed,
              .commit_on_release = true}, menu::order(60))
        .row({.key = "dlss5_local_structure",
              .label = "Local structure",
              .help = "[EXPERIMENTAL] Fine structure the model adds (NVIDIA's default 1). Makes the model again.",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_local_structure,
              .lo = 0, .hi = 200, .step = 2,
              .preset_exempt = true,
              .shown = tuning,
              .locked = strengths_locked,
              .text = hundredths,
              .on_change = changed,
              .commit_on_release = true}, menu::order(70))
        .row({.key = "dlss5_local_tone",
              .label = "Local tone",
              .help = "[EXPERIMENTAL] Local contrast and shading the model adds (NVIDIA's default 1). Makes the model "
                      "again.",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_local_tone,
              .lo = 0, .hi = 200, .step = 2,
              .preset_exempt = true,
              .shown = tuning,
              .locked = strengths_locked,
              .text = hundredths,
              .on_change = changed,
              .commit_on_release = true}, menu::order(80))
        .row({.key = "dlss5_skin",
              .label = "Skin structure",
              .help = "[EXPERIMENTAL] Structure on skin: Follows local structure, or 0 and above to set skin on its "
                      "own. Makes the model again.",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_skin,
              .lo = -100, .hi = 200, .step = 2,
              .preset_exempt = true,
              .shown = tuning,
              .locked = strengths_locked,
              .text = skin_text,
              .step_fn = skin_step,
              .on_change = changed,
              .commit_on_release = true,
              .slider_value = skin_slider_value}, menu::order(90))
        .row({.key = "dlss5_auto_mask",
              .label = "Automatic skin mask",
              .help = "[EXPERIMENTAL] The model finds skin by itself to treat it apart. Makes the model again.",
              .kind = menu::Kind::Toggle,
              .value = &opts.dlss5_auto_mask,
              .lo = 0, .hi = 1,
              .preset_exempt = true,
              .shown = tuning,
              .on_change = changed}, menu::order(100))
        .row({.key = "dlss5_highlight",
              .label = "Highlight guard",
              .help = "[EXPERIMENTAL] The most the model may brighten or darken a pixel, as a multiple: keeps bright "
                      "lights from breaking up. Applies at once.",
              .kind = menu::Kind::Slider,
              .value = &opts.dlss5_highlight,
              .lo = 10, .hi = 80, .step = 1,
              .preset_exempt = true,
              .shown = tuning,
              .text = guard_text,
              .on_change = changed}, menu::order(110));
}

} // namespace mkwvk
