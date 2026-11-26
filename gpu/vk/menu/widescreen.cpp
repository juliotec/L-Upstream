// The Insert menu's widescreen rows (#1633), in the Graphics tab's Screen section: where the game's HUD in play sits on a
// screen that is not 16:9. (Ultrawide menus, where its menus sit, is the game's: game/menu/rows.c.)
#include "menu_module.h"

#include <atomic>
#include <string>

#include "vk_api.h"

namespace mkwvk {

namespace {
/// (the HUD layout row, HOS_OPT_HUD_LAYOUT: game/hos_wide_ui.c places the HUD by it at each present, #1133)
std::atomic<int> hud_layout{0};
std::atomic<int> hud_health{HOS_HUD_DEFAULT_PLACEMENT};
std::atomic<int> hud_buttons{HOS_HUD_DEFAULT_PLACEMENT};
std::atomic<int> hud_minimap{HOS_HUD_DEFAULT_PLACEMENT};

int pack_placement(int x, int y, int scale) { return (x + 16) | ((y + 16) << 6) | (scale << 12); }

const char* tx_hud_layout(int v) {
    static const char* const values[] = {"Screen's edges", "Middle 16:9", "Custom"};
    return values[std::clamp(v, 0, 2)];
}
const char* hn_hud_layout(int v, int variant) {
    static const char* const t[] = {"The hearts, the buttons and the minimap at the screen's own edges and corners.",
                                    "The HUD where it is at 16:9, in the middle (fitted to the width on a narrower screen).",
                                    "Move and resize the hearts, buttons and minimap from their usual places."};
    static const char* const held = "Set by HOS_WIDE_UI (longshot_env.txt or the environment): remove that line to choose here.";
    const char* env = getenv("HOS_WIDE_UI");
    if (variant < 0) return env && *env ? held : t[std::clamp(v, 0, 2)];
    return variant == 0 ? t[std::clamp(v, 0, 2)] : variant == 1 && env && *env ? held : nullptr;
}

bool custom_layout() {
    const char* env = getenv("HOS_WIDE_UI");
    return opts.hud_layout == 2 && !(env && *env);
}

#ifndef MENU_LAYOUT_ONLY
void reset_hud_placement() {
    opts.hud_health_x = opts.hud_health_y = opts.hud_buttons_x = opts.hud_buttons_y = 0;
    opts.hud_minimap_x = opts.hud_minimap_y = 0;
    opts.hud_health_scale = opts.hud_buttons_scale = opts.hud_minimap_scale = 100;
    menu_sync_hud_placement();
    menu_save_options();
}
#endif
} // namespace

void menu_sync_hud_placement() {
    hud_health.store(pack_placement(opts.hud_health_x, opts.hud_health_y, opts.hud_health_scale), std::memory_order_relaxed);
    hud_buttons.store(pack_placement(opts.hud_buttons_x, opts.hud_buttons_y, opts.hud_buttons_scale), std::memory_order_relaxed);
    hud_minimap.store(pack_placement(opts.hud_minimap_x, opts.hud_minimap_y, opts.hud_minimap_scale), std::memory_order_relaxed);
    hud_layout.store(opts.hud_layout, std::memory_order_release);
}

int menu_hud_option(int id) {
    switch (id) {
    case HOS_OPT_HUD_HEALTH: return hud_health.load(std::memory_order_relaxed);
    case HOS_OPT_HUD_BUTTONS: return hud_buttons.load(std::memory_order_relaxed);
    case HOS_OPT_HUD_MINIMAP: return hud_minimap.load(std::memory_order_relaxed);
    default: return hud_layout.load(std::memory_order_acquire);
    }
}

MENU_MODULE(widescreen) {
    menu::tab("graphics")
        .row({.key = "hud_layout",
              .label = "HUD layout",
              .help = "Where the gameplay HUD sits. Custom starts at the screen's edges and lets you place and resize its groups. "
                      "Changes at once.",
              .value = &opts.hud_layout,
              .lo = 0, .hi = 2,
              .preset_exempt = true,
              .text = [](int v) { return std::string(tx_hud_layout(v)); },
              .hint = hn_hud_layout,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(60))
        .row({.key = "hud_health_x", .label = "Hearts and magic horizontal",
              .help = "Move left or right as a share of the picture's width.", .kind = menu::Kind::Slider,
              .value = &opts.hud_health_x, .lo = -15, .hi = 15, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(61))
        .row({.key = "hud_health_y", .label = "Hearts and magic vertical",
              .help = "Move down or up as a share of the picture's height.", .kind = menu::Kind::Slider,
              .value = &opts.hud_health_y, .lo = -15, .hi = 15, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(62))
        .row({.key = "hud_health_scale", .label = "Hearts and magic size",
              .help = "Resize the hearts and magic together.", .kind = menu::Kind::Slider,
              .value = &opts.hud_health_scale, .lo = 75, .hi = 125, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(63))
        .row({.key = "hud_buttons_x", .label = "Action and item buttons horizontal",
              .help = "Move left or right as a share of the picture's width.", .kind = menu::Kind::Slider,
              .value = &opts.hud_buttons_x, .lo = -15, .hi = 15, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(64))
        .row({.key = "hud_buttons_y", .label = "Action and item buttons vertical",
              .help = "Move down or up as a share of the picture's height.", .kind = menu::Kind::Slider,
              .value = &opts.hud_buttons_y, .lo = -15, .hi = 15, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(65))
        .row({.key = "hud_buttons_scale", .label = "Action and item buttons size",
              .help = "Resize the action prompts and equipped item buttons.", .kind = menu::Kind::Slider,
              .value = &opts.hud_buttons_scale, .lo = 75, .hi = 125, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(66))
        .row({.key = "hud_minimap_x", .label = "Minimap horizontal",
              .help = "Move left or right as a share of the picture's width.", .kind = menu::Kind::Slider,
              .value = &opts.hud_minimap_x, .lo = -15, .hi = 15, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(67))
        .row({.key = "hud_minimap_y", .label = "Minimap vertical",
              .help = "Move down or up as a share of the picture's height.", .kind = menu::Kind::Slider,
              .value = &opts.hud_minimap_y, .lo = -15, .hi = 15, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(68))
        .row({.key = "hud_minimap_scale", .label = "Minimap size",
              .help = "Resize the minimap during gameplay.", .kind = menu::Kind::Slider,
              .value = &opts.hud_minimap_scale, .lo = 75, .hi = 125, .preset_exempt = true,
              .shown = custom_layout, .text = menu_text::percent,
              .on_change = [](int) { menu_sync_hud_placement(); }}, menu::after("fullscreen_monitor").order(69))
        .row({.key = "reset_hud_placement", .label = "Reset HUD placement",
              .help = "Restore the three HUD groups to their default positions and sizes.", .kind = menu::Kind::Action,
              .shown = custom_layout, .on_activate = MENU_HOOK(reset_hud_placement)}, menu::after("fullscreen_monitor").order(70));
}

} // namespace mkwvk
