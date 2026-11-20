// The Insert menu's widescreen rows (#1633), in the Graphics tab's Screen section: where the game's menus and its HUD in
// play sit on a screen that is not 16:9.
#include "menu_module.h"

#include <string>

namespace mkwvk {

namespace {
const char* tx_wide_menus(int v) {
    static const char* t[] = {"Full width", "Centred, blurred sides", "Centred, black bars"};
    return t[std::min(std::max(v, 0), 2)];
}
MODULE_HINTS(hn_wide_menus, 0, "The menus stretched over the whole screen, as before.",
             "The menus in the middle 16:9; beside them, what the game draws there, out of focus.",
             "The menus in the middle 16:9; black beside them.")
/// (the HUD layout row, HOS_OPT_HUD_LAYOUT: host/hos_wide_ui.c places the HUD by it at each present, #1133)
const char* tx_hud_layout(int v) { return v ? "Middle 16:9" : "Screen's edges"; }
const char* hn_hud_layout(int v, int variant) {
    static const char* const t[] = {"The hearts, the buttons and the minimap at the screen's own edges and corners.",
                                    "The HUD where it is at 16:9, in the middle (fitted to the width on a narrower screen)."};
    static const char* const held = "Set by HOS_WIDE_UI (longshot_env.txt or the environment): remove that line to choose here.";
    const char* env = getenv("HOS_WIDE_UI");
    if (variant < 0) return env && *env ? held : t[v ? 1 : 0];
    return variant == 0 ? t[v ? 1 : 0] : variant == 1 && env && *env ? held : nullptr;
}
} // namespace

MENU_MODULE(widescreen) {
    menu::tab("graphics")
        .row({.key = "ultrawide_menus",
              .label = "Ultrawide menus",
              .help = "With the Aspect ratio at the monitor's shape: the pause, item and map menus, and what shows beside them. Loading "
                      "screens and pre-rendered movies, which draw no world either, get the same sides. Changes at once.",
              .value = &opts.wide_menus,
              .lo = 0, .hi = 2,
              .preset_exempt = true,
              .text = [](int v) { return std::string(tx_wide_menus(v)); },
              .hint = hn_wide_menus}, menu::after("fullscreen_monitor").order(50))
        .row({.key = "hud_layout",
              .label = "HUD layout",
              .help = "On a screen that is not 16:9: where the hearts, buttons and minimap sit. Ultrawide menus places the menus; this, the "
                      "HUD in play. At 16:9 both are the same. Changes at once.",
              .value = &opts.hud_layout,
              .lo = 0, .hi = 1,
              .preset_exempt = true,
              .text = [](int v) { return std::string(tx_hud_layout(v)); },
              .hint = hn_hud_layout}, menu::after("fullscreen_monitor").order(60));
}

} // namespace mkwvk
