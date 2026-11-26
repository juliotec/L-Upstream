// (longshot) The overlays' three fonts (vk_imgui.cpp loads them for the renderer's menu), apart from the renderer's headers:
// vk_ui.h's drawing needs only these.
#pragma once

struct ImFont;

namespace mkwvk::ui {

struct Fonts {
    ImFont* body = nullptr;      // (rows, help, prompts)
    ImFont* strong = nullptr;    // (the selected row, values, buttons)
    ImFont* heading = nullptr;   // (tabs, titles, the wordmark)
};

} // namespace mkwvk::ui
