// The DLSS 5 module's (gpu/vk/menu/dlss5.cpp) that the rest of the menu uses (vk_menu.cpp menu_init): the renderer told what
// its rows hold, and the Default preset's values (tests/test_dlss5_menu.py).
#pragma once

namespace mkwvk {
/// The DLSS 5 rows' values to the renderer (vk_dlss.h dlss_nr_publish), with RX_VK_DLSS_NR_DEBUG's view for this run and the
/// DLSS 5 model row as the run started (it applies after a restart)
void dlss5_publish();
/// RX_VK_DLSS_NR_MODEL: auto (or 0), nvidia (1) or fp16 (2) over the DLSS 5 model row for this run; -1 when unset or not one
/// of them
int dlss5_model_env();
/// The Default preset: the rows as tuned in the game (Kokiri Forest, an RTX 5090 at 5120x2160 with DLAA), DLSS 5 itself Off.
/// A row moved by hand from one of these makes the DLSS 5 preset row say Custom
struct Dlss5Preset {
    int resolution = 65, detail = 164, colour = 126, style = 2, strengths = 2, intensity = 162, local_structure = 122,
        local_tone = 114, skin = 74, auto_mask = 1, highlight = 17;
};
inline constexpr Dlss5Preset kDlss5Default{};
} // namespace mkwvk
