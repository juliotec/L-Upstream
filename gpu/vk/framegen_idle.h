// SPDX-License-Identifier: GPL-3.0-or-later
// (longshot) When frame generation's images are let go (vk_framegen.cpp framegen_idle): after kIdlePresents presents in a row with
// it off. A present that recorded made frames counts as on whatever the row says by then: its set is busy until the display thread
// shows it, and a drain then would wait for a job that is only queued after (the hang #1158's review found: off for 119 presents, a
// present that records with it on and reads Off after). Nothing of Vulkan here: tests/test_framegen_idle.py runs it.
#pragma once
#include <cstdint>

namespace framegen_idle_rule {

constexpr uint32_t kIdlePresents = 120;   // (about 2 s at 60: a row stepped through on the way to another keeps them)

/// The presents in a row off, after this one: on now, or made frames recorded this present, starts again
inline uint32_t next_off(uint32_t off, bool on_now, bool recorded) { return on_now || recorded ? 0 : off + 1; }

/// Whether this present lets them go: there are some, and it has been off long enough
inline bool release_due(uint32_t off, bool has_images) { return has_images && off >= kIdlePresents; }

}  // namespace framegen_idle_rule
