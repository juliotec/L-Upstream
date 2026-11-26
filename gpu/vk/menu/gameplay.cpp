// The Insert menu's Game speed row (#1633), on the Gameplay tab: how the game keeps time below 60 fps (host/nn_vi.c, option
// 0; the Quality presets set it). The tab's rows of the game itself (Field of view, Text speed and size, Skip title screen,
// Skip opening, Discord status) are the game's: game/menu/rows.c, added by game_rows.cpp.
#include "menu_module.h"

#include <string>

namespace mkwvk {

namespace {
const char* tx_speed(int v) { return v ? "Real time" : "Per frame"; }
MODULE_HINTS(hn_speed, 0, "A step each frame: the game slows down whenever it runs below 60 fps.",
             "Normal speed down to 15 fps, by taking longer steps.")
} // namespace

MENU_MODULE(gameplay) {
    menu::tab("gameplay")
        .row({.key = "game_speed",
              .label = "Game speed",
              .help = "How the game keeps time when it runs below 60 fps. 30 FPS always uses real-time game speed, even when Game speed is Per frame.",
              .value = &opts.realtime,
              .lo = 0, .hi = 1,
              .text = [](int v) { return std::string(tx_speed(v)); },
              .hint = hn_speed}, menu::before("bug_report").order(300));
}

} // namespace mkwvk
