#include "../gpu/vk/dlss_nr_settings.h"
#include <cassert>
#include <cstdio>
using namespace mkwvk::dlss_nr;
int main() {
    Edits edits;
    Settings a;
    a.intensity = 100;
    assert(!edits.observe(a, 0));
    a.intensity = 150;
    assert(!edits.observe(a, 300));
    assert(!edits.observe(a, 649));
    assert(edits.observe(a, 650));
    edits.commit(650);
    assert(edits.applied.intensity == 150);
    a.tone = 50;
    assert(!edits.observe(a, 1000));
    assert(!edits.observe(a, 2649)); // Avoid flooding the driver's feature pool.
    assert(edits.observe(a, 2651));
    a = edits.applied; // Reverting an edit must cancel the pending recreation.
    assert(!edits.observe(a, 3000));
    assert(!edits.observe(a, 4000));
    a.strengths = 1; // The Model strengths row: a change makes the feature again too.
    assert(!edits.observe(a, 4010));
    assert(edits.observe(a, 4360));
    edits.commit(4360);
    a.intensity = 900; a.structure = -12; a.skin = -5; a.style = 99; a.preset = -1; a.strengths = 7;
    assert(!edits.observe(a, 6400));
    assert(edits.observe(a, 6750));
    edits.commit(6750);
    assert(edits.applied.intensity == 200 && edits.applied.structure == 0 && edits.applied.strengths == 1);
    assert(edits.applied.skin == -100 && edits.applied.style == 2 && edits.applied.preset == 0);
    assert(!edits.observe(a, 9000)); // Clamping must not cause an endless rebuild loop.
    // The model's working size: a percent of the picture, a multiple of 8, at least 8, at most the picture.
    assert(extent(3416, 100) == 3416 && extent(1440, 100) == 1440);
    assert(extent(3416, 50) == 1712 && extent(1440, 50) == 720);
    assert(extent(3416, 25) == 856 && extent(20, 25) == 8 && extent(6, 25) == 6);
    assert(extent(1440, 10) == extent(1440, 25) && extent(1440, 150) == 1440);
    // The model resolution row stepped 100 -> 65 by its arrows: one size taken, once it held for 350 ms.
    Settled scale;
    assert(scale.take(100, 0) == 100);   // (the first takes at once)
    int value = 100;
    for (int64_t t = 100; value > 65; t += 100) {
        value -= 5;
        assert(scale.take(value, t) == 100);   // (each step within 350 ms of the last: still the old size)
    }
    assert(scale.take(65, 649) == 100 && scale.take(65, 1050) == 65);
    assert(scale.take(70, 2000) == 65 && scale.take(65, 2100) == 65 && scale.take(65, 5000) == 65);   // (back before it took)
    std::puts("NR edit coalescing, rate limit, cancellation, bounds, working size and its settle: PASS");
}
