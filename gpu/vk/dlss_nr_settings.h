// DLSS 5's model settings (vk_dlss.cpp neural_render): latched when its feature is made, so an edit makes the feature
// again, once the edits settle (Edits). The menu's rows (gpu/vk/menu/dlss5.cpp) give them; no GPU here.
#pragma once

#include <algorithm>
#include <cstdint>

namespace mkwvk::dlss_nr {
/// Hundredths where NVIDIA's parameter is a float 0 to 2; skin -100 follows the local structure. strengths 0: the four
/// strengths are not sent and the model keeps its own; 1: they are sent (the menu's Model strengths row, Applied)
struct Settings {
    int intensity = 100, structure = 100, tone = 100, skin = 100;
    int style = 0, preset = 0, mask = 1, ui = 1, strengths = 0;
    bool operator==(const Settings&) const = default;
    Settings clamped() const {
        return {std::clamp(intensity, 0, 200), std::clamp(structure, 0, 200),
                std::clamp(tone, 0, 200), skin < 0 ? -100 : std::clamp(skin, 0, 200),
                std::clamp(style, 0, 2), std::clamp(preset, 0, 3), mask != 0, ui != 0, strengths != 0};
    }
};

/// The model's working size on one side: a percent of the picture's, a multiple of 8, at least 8 and at most the picture
inline uint32_t extent(uint32_t full, int scale) {
    scale = std::clamp(scale, 25, 100);
    if (scale == 100) return full;
    return std::min(full, std::max(8u, ((full * (uint32_t)scale / 100u) + 7u) & ~7u));
}

/// Model edits are coalesced before the feature is made again
struct Edits {
    Settings pending{}, applied{};
    int64_t changed_ms = 0, applied_ms = -2000;
    bool observe(Settings wanted, int64_t now) {
        wanted = wanted.clamped();
        if (!(pending == wanted)) pending = wanted, changed_ms = now;
        return !(applied == wanted) && now - changed_ms >= 350 && now - applied_ms >= 2000;
    }
    void commit(int64_t now) { applied = pending; applied_ms = now; }
};

/// The model resolution row's percent, settled: a change takes once it has held for 350 ms, as the model's other settings
/// do, so the row's arrows stepped from 100% to 65% make the model once, at the size they stop at, not at each 5% step.
/// The first value takes at once
struct Settled {
    int applied = -1, wanted = -1;
    int64_t since = 0;
    int take(int value, int64_t now) {
        if (applied < 0) return applied = wanted = value;
        if (value != wanted) wanted = value, since = now;
        if (wanted != applied && now - since >= 350) applied = wanted;
        return applied;
    }
};
} // namespace mkwvk::dlss_nr
