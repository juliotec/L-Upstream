"""A fixed frame generation multiplier's frames at their times on the game's own timeline wherever the swapchain does not
wait for a refresh (V-Sync Off), whatever Frame pacing says.

With Frame pacing Off (its default) the display thread presented a set's frames as they came: within a millisecond of
each other at V-Sync Off, so the screen showed the game's frame and hardly the made ones, and DLSS Frame Gen 3x looked like
the game's own frame rate. The real fixed_dues, pace_lead, pace_delay and fixed_on_timeline of gpu/vk/vk_framegen.cpp:

- each shown frame of a 3x set is shown as far after the one before as its picture is after the one before's, across
  sets too, with the game's frames 15.5 to 22 ms apart (as at ~55 fps);
- the display loop, modelled: sets made at the game's times, their frames done 12 ms after their game frame (DLAA at
  5120x2160), shown one after the other at their times, each set's need fed back when it is shown. The GPU's delay settles
  without running away, every frame is then shown on time and 4 to 9 ms after the one before (never two together), a
  long game frame (60 ms) does not raise it, and it comes back down once the GPU has room;
- at 4x with an NVIDIA surface's 6 spare images (two sets need 8): the next set's copies as images free, its first at
  once, and only sets whose first copy went as they came fed back: the delay settles and every frame is on time. As
  before (a set's copies only once all fit, every set fed back) it ran to a whole game frame;
- the delay never passes a game frame, and a hitch leaves it;
- only a set with its times on a swapchain that is not FIFO is shown at them, and RX_VK_FG_TIMELINE=0 turns it off;
- present_image's frame slots: 3, and a slot per frame of two sets where the swapchain is made for two (V-Sync Off,
  RX_VK_FG_TIMELINE on) while frame generation runs: a set's copies all at once, none waiting in its own first copy's
  slot or in the slot of the shown set's copy; FIFO and RX_VK_FG_TIMELINE=0 keep 3.

The text checks: every fixed set with a frame before (blend) and a made frame gets its times, whatever Frame pacing says;
display_main decides from the swapchain's mode, puts only a set presented ahead on the timeline, takes the next one early
(looked for every 2 ms of a wait) with its copies for the images free, the rest as presents give images back, and feeds
back only a set whose first copy it waited for, that went as it came and that did not follow a long game frame.
vk_main.cpp asks for two sets' images where the swapchain does not wait, one set's with RX_VK_FG_TIMELINE=0.
python -I -B tests/test_framegen_fixed_pace.py [--cxx g++]
"""
import argparse
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from srccut import block  # noqa: E402

RUN_ALL_ARGS = ["--cxx", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]

PREFIX = r'''#include <algorithm>
#include <initializer_list>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#define CHECK(x) do { if (!(x)) { std::fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); std::exit(1); } } while (0)
'''

MAIN = r'''
/* The display loop as display_main runs it for sets on the timeline, presented ahead and taken early: set i's game frame
   comes at now; its copy k goes to the GPU once the swapchain has an image for it beside those the set before still holds
   (its frames not yet presented: o.spare images in all), at once where it has, and is done gpu ns after it went (behind
   the game's work submitted meanwhile). o.whole: a set's copies only once all of them fit, as before. The display thread
   shows the sets in order, each frame at its time or once its copy is done, and feeds a set's need back once its first
   copy is done, unless it followed a long game frame or its first copy did not go as it came (o.feed_all: every set's, as
   before). r.extra: the GPU's delay at the end; r.shown and r.due: every present's time and its due time (the first set
   left out) */
struct Opt { int spare = 64; bool whole = false, feed_all = false; };
struct Run { int64_t extra; int n; int64_t shown[4096]; int64_t due[4096]; };
static void run(const int64_t* gaps, int sets, int64_t gpu, int made, Run& r, int64_t start_extra = 0, Opt o = {}) {
    int64_t last = 0, game_ns = 0, extra = start_extra, display = 0;
    int64_t before[8] = {};   /* when the set before's frames were shown */
    int before_n = 0;
    r.n = 0;
    for (int i = 0; i < sets; i++) {
        const int64_t now = last + gaps[i];
        if (last && gaps[i] <= 100000000) game_ns = game_ns ? (game_ns * 7 + gaps[i]) / 8 : gaps[i];   /* framegen_record */
        int64_t due[8];
        const int64_t set_extra = extra;
        fixed_dues(last, now, made, pace_lead(game_ns, made) + set_extra, due);
        int64_t went[8];
        for (int k = 0; k <= made; k++) {
            const int images = o.whole ? made + 1 : k + 1;           /* this set's, with copy k */
            const int presented = before_n + images - o.spare;        /* of the set before's, first */
            went[k] = presented > 0 ? std::max(now, before[presented - 1]) : now;
        }
        const int64_t copied = went[0] + gpu;                         /* its first copy done */
        const bool as_came = went[0] - now <= 3000000;
        if (i > 0 && gaps[i] * 2 <= game_ns * 3 && (as_came || o.feed_all))
            extra = pace_delay(extra, set_extra + (copied - due[0]) + 1000000, game_ns);
        int64_t t = display;                                          /* shown once the set before was */
        for (int k = 0; k <= made; k++) {
            t = std::max({t, due[k], went[k] + gpu});
            before[k] = t;
            if (i > 0 && r.n < 4096) r.due[r.n] = due[k], r.shown[r.n++] = t;
        }
        before_n = made + 1;
        display = t;
        last = now;
    }
    r.extra = extra;
}
/* frames shown more than 1 ms late, and closer than `close` to the one before, in the second half */
static void count(const Run& r, int64_t close, int& late, int& together) {
    late = together = 0;
    for (int i = r.n / 2; i < r.n; i++) {
        late += r.shown[i] > r.due[i] + 1000000;
        together += r.shown[i] - r.shown[i - 1] < close;
    }
}

int main() {
    const int64_t ms = 1000000;
    /* the game's frames 15.5 to 22 ms apart, 3x: two made frames and the game's a set */
    const int64_t gaps[] = {16 * ms, 22 * ms, 15500000, 19 * ms, 21 * ms, 17 * ms, 22 * ms, 16 * ms};
    int64_t last = 1000 * ms, prev_due = 0, prev_content = 0;
    const int64_t delay = 13 * ms;
    for (int64_t g : gaps) {
        const int64_t now = last + g;
        int64_t due[8];
        fixed_dues(last, now, 2, delay, due);
        CHECK(due[2] == now + delay);                                  /* the game's frame one delay after it came */
        for (int k = 0; k < 3; k++) {
            const int64_t content = last + g * (k + 1) / 3;            /* where frame k's picture is between the two */
            CHECK(due[k] - content == delay);                          /* shown a fixed time after its picture */
            if (prev_due) {
                const int64_t shown = due[k] - prev_due, moved = content - prev_content;
                CHECK(shown == moved);                                 /* each shown frame moves as far as its time */
                CHECK(shown >= 5 * ms && shown <= 8 * ms);             /* a third of 15.5 to 22 ms: never together */
            }
            prev_due = due[k], prev_content = content;
        }
        last = now;
    }
    int64_t due[8];
    fixed_dues(0, 30 * ms, 1, 0, due);                                 /* 2x: the midpoint, then the game's frame */
    CHECK(due[0] == 15 * ms && due[1] == 30 * ms);
    fixed_dues(0, 40 * ms, 3, 5 * ms, due);                            /* 4x: quarters */
    CHECK(due[0] == 15 * ms && due[1] == 25 * ms && due[2] == 35 * ms && due[3] == 45 * ms);

    /* the lead: at the game's own frame time the set's first frame is due when its game frame came, no earlier */
    CHECK(pace_lead(18 * ms, 2) == 12 * ms && pace_lead(18 * ms, 1) == 9 * ms && pace_lead(20 * ms, 3) == 15 * ms);
    CHECK(pace_lead(18 * ms, 0) == 0);
    for (int made = 1; made <= 3; made++) {
        const int64_t frame = 18 * ms, now = 500 * ms;
        fixed_dues(now - frame, now, made, pace_lead(frame, made), due);
        CHECK(due[0] == now);
        int64_t next[8];                                                /* the next set, a game frame later */
        fixed_dues(now, now + frame, made, pace_lead(frame, made), next);
        CHECK(next[0] - due[made] == frame / (made + 1));               /* its first frame one step after this game frame */
    }

    /* the display loop: 600 sets at 15.5 to 22 ms (~55 fps), 3x, frames done 12 ms after their game frame */
    static int64_t g600[600];
    for (int i = 0; i < 600; i++) g600[i] = gaps[i % 8];
    static Run r;
    run(g600, 600, 12 * ms, 2, r);
    CHECK(r.extra >= 13 * ms && r.extra <= 16 * ms);   /* settled: the GPU's 12 ms, 1 to spare, the frames' unevenness */
    int late = 0, together = 0, apart = 0;
    for (int i = 300; i < r.n; i++) {                                    /* the second half, settled */
        late += r.shown[i] > r.due[i] + 1000000;
        const int64_t d = r.shown[i] - r.shown[i - 1];
        together += d < 4 * ms;
        apart += d > 9 * ms;
    }
    CHECK(late == 0);                                                    /* every frame on time */
    CHECK(together == 0 && apart == 0);                                  /* 4 to 9 ms apart: never two together */
    const int64_t settled = r.extra;
    run(g600, 600, 30 * ms, 2, r);                                       /* a GPU slower than a game frame */
    CHECK(r.extra <= 22 * ms);                                           /* held to a game frame (the smoothed ~18.5 ms) */
    /* a long game frame (60 ms) among them: the sets after it do not raise the delay */
    static int64_t hitch[600];
    for (int i = 0; i < 600; i++) hitch[i] = i == 400 ? 60 * ms : g600[i];
    run(hitch, 600, 12 * ms, 2, r);
    CHECK(r.extra <= settled + ms);
    /* the GPU with room again: the delay comes back down */
    run(g600, 600, 2 * ms, 2, r, settled);
    CHECK(r.extra <= 5 * ms);                                            /* 2 ms, 1 to spare, the unevenness */

    /* 4x on an NVIDIA surface: 6 spare images, two sets need 8. As before (a set's copies once all fit, every set fed back)
       the next set waited for the images of the one before once the delay passed 3/4 of a game frame, its copies went later,
       the delay rose with them: up to a whole game frame. Now its first copies go at once and a set that waited is left out */
    run(g600, 600, 12 * ms, 3, r);                                       /* 4x with images to spare */
    const int64_t free4 = r.extra;
    CHECK(free4 >= 13 * ms && free4 <= 16 * ms);
    run(g600, 600, 12 * ms, 3, r, 0, Opt{6, true, true});                /* as before */
    CHECK(r.extra >= 17 * ms);                                           /* nearly a game frame: it fed itself */
    const Opt nvidia{6, false, false};
    for (int64_t gpu : {12 * ms, 14 * ms}) {
        run(g600, 600, gpu, 3, r, 0, nvidia);
        int late4 = 0, together4 = 0;
        count(r, 3 * ms, late4, together4);                              /* a quarter of 15.5 to 22 ms: 3.9 to 5.5 */
        CHECK(late4 == 0 && together4 == 0);
        if (gpu == 12 * ms) CHECK(r.extra <= free4 + ms);                /* as if the images were there */
        CHECK(r.extra <= gpu + 3 * ms);                                  /* the GPU's, 1 to spare, the unevenness */
    }

    /* present_image's frame slots for a swapchain's sets: 3; where it is made for two sets of a fixed multiplier, a slot per
       frame of both (no copy waits in the slot of its own set's first copy or of the shown set's copy), at most
       kPresentSlotsMax; one set (FIFO, RX_VK_FG_TIMELINE=0): 3, as before */
    CHECK(present_slots_for(0, true) == 3 && present_slots_for(1, true) == 3);    /* none; Auto */
    CHECK(present_slots_for(2, true) == 4 && present_slots_for(3, true) == 6 && present_slots_for(4, true) == 8);
    CHECK(present_slots_for(2, false) == 3 && present_slots_for(3, false) == 3 && present_slots_for(4, false) == 3);
    CHECK(present_slots_for(99, true) == kPresentSlotsMax);
    for (int m = 2; m <= 4; m++) CHECK(present_slots_for(m, true) >= 2 * m);    /* both sets' frames, up to 4x (8 images) */

    /* the delay's limits: never past a game frame, a hitch (a need past four frames) leaves it, no frame time yet leaves it */
    const int64_t frame = 18 * ms;
    int64_t big = 0;
    for (int i = 0; i < 40; i++) big = pace_delay(big, 3 * frame, frame);
    CHECK(big == frame);
    CHECK(pace_delay(5 * ms, 100 * ms, frame) == 5 * ms);
    CHECK(pace_delay(5 * ms, 9 * ms, 0) == 5 * ms);
    CHECK(pace_delay(0, -10 * ms, frame) == 0);                          /* early with none: none */
    CHECK(pace_delay(8 * ms, 12 * ms, frame) == 10 * ms);                /* up by half of what is missing */
    CHECK(pace_delay(8 * ms, 0, frame) == 7500000);                      /* down by a sixteenth of what is over */

    /* where a set is shown at its times: with them, on a swapchain that is not FIFO, unless RX_VK_FG_TIMELINE=0 */
    CHECK(fixed_on_timeline(true, false, false));
    CHECK(!fixed_on_timeline(true, true, false));                        /* V-Sync On: FIFO shows one a refresh */
    CHECK(!fixed_on_timeline(true, false, true));                        /* RX_VK_FG_TIMELINE=0 */
    CHECK(!fixed_on_timeline(false, false, false));                      /* no times: the first set, a hitch, no made frame */
    std::printf("PASS: a fixed multiplier's frames go out at their own times at V-Sync Off, and wait only as the GPU asks\n");
}
'''


def check_sources(source, main_cpp):
    record = block(source, "bool framegen_record(", where="gpu/vk/vk_framegen.cpp")
    display = block(source, "void display_main()", where="gpu/vk/vk_framegen.cpp")
    assert re.search(r"set->shown = -1;\s*set->by_time = false;", record), \
        "framegen_record: a reused set starts without times"
    m = re.search(r"if \(blend && set->made > 0 && fg\.game_ns > 0\) \{(.*?)\n    \}", record, re.S)
    assert m, "framegen_record: every fixed set with a frame before and a made frame gets its times"
    body = m.group(1)
    for part in ("fixed_dues(before, now, set->made, pace_lead(fg.game_ns, set->made) + extra, set->due);",
                 "set->span_ns = now - before;", "set->extra_ns = extra;", "set->by_time = true;",
                 "set->game[k] = k == set->made;"):
        assert part in body, "framegen_record's times: " + part
    assert "set->shown" not in body, "framegen_record: a fixed set keeps shown < 0; display_main decides on its times"
    assert "opts.pace" not in body, "framegen_record: the times do not depend on Frame pacing"
    assert record.index("if (auto_rate)") < record.index("fixed_dues("), "framegen_record: Auto keeps its own slots"
    assert record.index("note_maker(maker);") < record.index("fixed_dues("), \
        "framegen_record: the set's made frames are known before its times"

    assert 'getenv("RX_VK_FG_TIMELINE")' in display, "display_main: RX_VK_FG_TIMELINE=0 turns the times off"
    assert re.search(r"set->shown < 0 && fixed_on_timeline\(set->by_time, dev\.swap_mode == VK_PRESENT_MODE_FIFO_KHR, "
                     r"timeline_off\)", display), "display_main: the swapchain's mode decides, not Frame pacing"
    assert re.search(r"int64_t due = timed \? j\.set->due\[k\]", display), "display_main: each frame at its own due time"
    take = block(display, "const auto take = [&](Taken& s, const Job& j, int room)", where="gpu/vk/vk_framegen.cpp")
    order = [take.index(t) for t in ("s.ahead = present_ahead_wanted(pace || line, s.shown,",
                                     "s.line = line && s.ahead;", "s.timed = s.auto_timed || s.line;",
                                     "copy_up_to(s, room);")]
    assert order == sorted(order), \
        "take: a set on the timeline is presented ahead with Frame pacing Off too, and on it only presented ahead"
    assert "const bool line = on_line(j.set);" in take, "take: the swapchain's mode decides, not Frame pacing"
    copy = block(display, "const auto copy = [&](Taken& s, int k)", where="gpu/vk/vk_framegen.cpp")
    assert "if (k == 0) s.as_came = t0 - j.queued <= 3000000 && now_ns() - t0 <= 3000000;" in copy and \
        "const int64_t t0 = now_ns();" in copy and "as_came" not in take, \
        "copy: the set taken within 3 ms of its coming, its first copy not held over 3 ms in present_image"
    assert "fg.jobs.push_back({fg.recorded, fg.recorded_at, value, now_ns()});" in source, \
        "framegen_queue: the time a set is handed to the display thread"
    m = re.search(r"if \(s\.line\) \{(.*?)\n        \}\n", display, re.S)
    assert m, "display_main: the feedback of a set on the timeline"
    feed = m.group(1)
    assert re.search(r"if \(first && s\.as_came\) \{.*pace_delay\(", feed, re.S) and \
        "const bool first = s.ahead && s.pending[0].used;" in display, \
        "display_main: only a set whose first copy was waited for and went as it came feeds the GPU's delay back"
    assert "const int64_t at = s.ready_at ? s.ready_at : now_ns();" in feed, \
        "display_main: the copy's end as seen while the set before was shown, else now"
    assert "j.set->span_ns * 2 <= j.set->frame_ns * 3" in feed, \
        "display_main: a set after a long game frame does not feed it back"
    assert "j.set->extra_ns + (at - j.set->due[0]) + 1000000" in feed, \
        "display_main: the need is the set's own delay and its lateness, a millisecond to spare"
    loop = display[display.index("    for (;;) {"):]
    assert loop.index("copy_up_to(s, (int)dev.swap_spare.load(std::memory_order_relaxed));") < \
        loop.index("const bool first = ") < loop.index("vkWaitForFences") < loop.index("pace_delay("), \
        "display_main: a set taken early has its last copies sent, then the lateness once its first copy is done"
    present_loop = loop.index("for (int k = 0; k < s.shown; k++) {")
    assert loop.index("pace_delay(") < present_loop, "display_main: the lateness is taken before the set's first present"
    assert present_loop < loop.index("take_early(s.shown - k, due);") < loop.index("wait_until_near("), \
        "display_main: the next set is taken while this one waits for its frames' times"
    assert re.search(r"for \(;;\) \{\s*take_early\(s\.shown - k, due\);.*?std::min<int64_t>\(due - now - 1000000, 2000000\)",
                     loop, re.S), \
        "display_main: the wait looks for the next set every 2 ms at most (at 2x it comes during the game frame's wait)"
    early = block(display, "const auto take_early = [&](int left, int64_t until)", where="gpu/vk/vk_framegen.cpp")
    assert "shown > spare || spare - left < 1" in early and "take(taken[1], j, spare - left);" in early, \
        "take_early: a set the swapchain can hold, its copies for the images this set does not hold"
    assert "!on_line(j.set)" in early and "!taken[0].line" in early, \
        "take_early: only a set on the timeline after one on it (presented ahead)"
    assert "taken[1].ready_at = now_ns();" in loop, "display_main: a set taken early has its first copy's end seen"
    assert "copy_up_to(taken[1], (int)dev.swap_spare.load(std::memory_order_relaxed) - (s.shown - k));" in loop, \
        "display_main: the next set's other copies as this set's presents give images back"
    assert "(t - (s.line ? own : due) > 2000000 ? stat.late : stat.on_time)++;" in loop, \
        "display_main: a frame on the timeline is late against its own time"

    chain = block(main_cpp, "bool create_swapchain(", where="gpu/vk/vk_main.cpp")
    assert "const uint32_t fg_sets = mode == VK_PRESENT_MODE_FIFO_KHR || !timeline ? 1 : 2;" in chain and \
        'getenv("RX_VK_FG_TIMELINE")' in chain and \
        "caps.minImageCount + std::min<uint32_t>(fg_sets * (uint32_t)swapchain_framegen()," in chain and \
        "(uint32_t)kPresentSlotsMax));" in chain, \
        "create_swapchain: two sets' images where the swapchain does not wait for a refresh, one set's under FIFO and " \
        "with RX_VK_FG_TIMELINE=0"
    assert "const uint32_t slots_made = (uint32_t)present_slots_for(swapchain_framegen(), fg_sets == 2);" in chain and \
        "dev.swap_slots = slots_made;" in chain and \
        chain.index("dev.swap_slots = slots_made;") > chain.index("retire_swapchain();"), \
        "create_swapchain: present_image's frame slots for the sets the chain is made for, stored after the old chain's " \
        "retire_swapchain (which sets 3)"


def check_slots(replay, internal, imgui):
    m = re.search(r"constexpr int kPresentSlotsMax = (\d+);", internal)
    assert m, "vk_internal.h: kPresentSlotsMax"
    assert "Frame frames[kPresentSlotsMax];" in replay and "LogSlot log_slots[kPresentSlotsMax];" in replay, \
        "vk_replay.cpp: the frame slots and their frame-log slots, kPresentSlotsMax of each"
    image = block(replay, "void present_image(VkImage src,", where="gpu/vk/vk_replay.cpp")
    for part in ("const uint32_t slots = framegen_on() ? std::clamp<uint32_t>(dev.swap_slots.load(std::memory_order_relaxed), 3u,",
                 "const uint32_t slot = frame_index % slots;", "Frame& f = frames[slot];",
                 "frame_index = (slot + 1) % slots;", "for (uint32_t k = slots; k < slots_was; k++)"):
        assert part in image, "present_image: " + part
    assert "std::atomic<uint32_t> swap_slots{3};" in internal, "vk_internal.h: the swapchain's frame slots, 3 to start"
    assert "ii.ImageCount = kPresentSlotsMax + 1;" in imgui, \
        "vk_imgui.cpp: ImGui's vertex buffers, one more than the presents in flight"
    return int(m.group(1))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cxx", default="g++")
    a = p.parse_args()
    vk = ROOT / "gpu" / "vk"
    source = (vk / "vk_framegen.cpp").read_text(encoding="utf-8")
    main_cpp = (vk / "vk_main.cpp").read_text(encoding="utf-8")
    replay = (vk / "vk_replay.cpp").read_text(encoding="utf-8")
    internal = (vk / "vk_internal.h").read_text(encoding="utf-8")
    imgui = (vk / "vk_imgui.cpp").read_text(encoding="utf-8")
    slots_max = check_slots(replay, internal, imgui)
    check_sources(source, main_cpp)
    fixture = (PREFIX + f"constexpr int kPresentSlotsMax = {slots_max};\n" +
               "\n".join(block(source, m, where="gpu/vk/vk_framegen.cpp") for m in (
                   "void fixed_dues(", "int64_t pace_lead(", "int64_t pace_delay(", "bool fixed_on_timeline(")) + "\n" +
               block(replay, "int present_slots_for(", where="gpu/vk/vk_replay.cpp") + MAIN)
    with tempfile.TemporaryDirectory(prefix="framegen-fixed-pace-") as tmp:
        cpp = Path(tmp) / "fixture.cpp"
        exe = Path(tmp) / "fixture"
        cpp.write_text(fixture, encoding="utf-8")
        subprocess.run([a.cxx, "-std=c++20", "-Wall", "-Wextra", "-Werror", str(cpp), "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == "__main__":
    main()
