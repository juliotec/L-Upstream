"""A Frame rate cap above 60 steps the game by the frames' own time, so the game is not sped up.

The engine's rate is a whole number of display intervals, 60 at most, and it puts 60 back over a higher one: with only the
rate raised, every frame above 60 fps still stepped 1/60 s. The engine's two step divides are hook sites with inline data
rules (frame.stepDivide, frame.globalStepDivide: game/sites/sites.toml, the rule mul_f32), which put base * the
host's measured frame time (hos_frame_seconds_bits, sites.def's frame.seconds) in place of the quotient, and
host/nn_vi.c realtime_step publishes it while the cap is above 60 and the hook is in the build (frame.stepProbe's note_u64
sets hos_step_probe); Uncapped (frame_rate_cap=6) is a cap of 1000, whose frames step by their own time down to
1 ms, so the game keeps real time past 240 fps. The row applies in play (HOS_OPT_FPS_CAP): a cap brought down gives the game
back its own limiter, rate and step. At a cap of 60 the bits stay 0 and the lifted code runs as shipped. The rule as the lift lowers it (tests/sitehooks.py) runs here after a divide
as the lift writes it, and against the text patch it replaced (kept below as main had it).

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_frame_rate_cap_step.py [--cc clang] [--cargo cargo]
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from menu_model_fixture import menu_text  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from sitecheck import host_sites, host_values, load  # noqa: E402
import sitehooks  # noqa: E402

# (env_on, the switches' rule, before the code cut out of the sources: gpu/vk/compat/longshot_env.h)
ENV_ON_H = str(Path(__file__).resolve().parents[1] / "gpu" / "vk" / "compat" / "longshot_env.h")

BUILD = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348"
# (the text patch's divide on main: the quotient, then base * the frame's seconds in its place while the host gives them)
OLD_HOOK = ("    _v=a64_fpx_div32_div32(c,_a,_b);\n"
            "    { const uint32_t _hs = hos_frame_seconds_bits; if (_hs) { float _base, _secs; uint32_t _w; memcpy(&_base,&_a,4); "
            "memcpy(&_secs,&_hs,4); _base *= _secs; memcpy(&_w,&_base,4); _v = _w; } }\n")


def block(source, marker):
    start = source.index(marker)
    return source[start:start + re.search(r"^}", source[start:], re.M).end()]


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


FIXTURE = r'''#define _CRT_SECURE_NO_WARNINGS
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <stddef.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
typedef void Ctx;
static int present_interval = 1;
static uint8_t guest[0x4000];
#define GP(a) ((void *)(guest + (a)))
#define MOD 0x1000u
static int clock_logs;
static long long clock_now;
static int64_t now = 1000000000LL;   /* (the presents' clock; hos_now_ns below reads it) */
static void hos_log(const char *fmt, ...) {
    char text[256];
    va_list args; va_start(args, fmt); vsnprintf(text, sizeof text, fmt, args); va_end(args);
    if (!strncmp(text, "vi: game clock ", 15)) {
        const char *stamp = strstr(text, "; now_ns=");
        CHECK(stamp && sscanf(stamp, "; now_ns=%lld", &clock_now) == 1);
        clock_logs++;
    }
}
int hos_ini_option(const char *key, int otherwise) {   /* (frame_rate_cap=: the test's T_CAP_INDEX) */
    const char *e = getenv("T_CAP_INDEX");
    return !strcmp(key, "frame_rate_cap") && e ? atoi(e) : otherwise;
}
volatile uint64_t hos_gfx_module = MOD;
volatile int hos_fixed_resolution, hos_dynres_runs, hos_scene_rgba16f;
volatile uint32_t hos_fixed_scale_bits;
volatile float hos_text_scale, hos_camera_scale;
@OPTS@
static int live_cap = -2;   /* (the Game frame rate cap row changed in play: HOS_OPT_FPS_CAP's answer, -1 the renderer not up) */
int rx_option(int id, int otherwise) { return id == 0 && getenv("T_PER_FRAME") ? 0 : id == HOS_OPT_FPS_CAP && live_cap >= -1 ? live_cap : otherwise; }
volatile uint64_t hos_step_probe, hos_res_module;
volatile uint32_t hos_frame_seconds_bits;
/* (the host clock the extracted cap code reads for the movie pacing; movie_seen_at itself comes with the
   extracted block, which starts at the cap's enum) */
static int64_t hos_now_ns(void) { return now; }
@HOST@
static uint64_t a64_fpx_div32_div32(void *c, uint64_t a, uint64_t b) {
    float x, y; uint32_t w;
    (void)c; memcpy(&x, &a, 4); memcpy(&y, &b, 4); x /= y; memcpy(&w, &x, 4);
    return w;
}
/* (the lifted divide, fdiv s8, s1, s0, then the site's rule as the lift lowers it (tests/sitehooks.py): base / nominal, or base * the frame's
   seconds, in a context of the lifted code's layout (the vector registers at 272); and the text patch's divide on main, which must give the same) */
static float lifted_step(float base, float nominal) {
    struct { uint64_t x[32], pc; uint8_t n, z, c, v; int halted; uint64_t vreg[32][2]; uint8_t rest[80]; } ctx, *c = &ctx;
    _Static_assert(offsetof(__typeof__(ctx), vreg) == 272, "the lifted code's context");
    float r, was;
    memset(&ctx, 0, sizeof ctx);
    memcpy(&c->vreg[1][0], &base, 4), memcpy(&c->vreg[0][0], &nominal, 4);
    {
        uint64_t _a = 0, _b = 0, _v;
        memcpy(&_a, c->vreg[1], 4); memcpy(&_b, c->vreg[0], 4);
        _v = a64_fpx_div32_div32(c, _a, _b);
        c->vreg[8][0] = _v, c->vreg[8][1] = 0;
    }
@HOOK@
    memcpy(&r, &c->vreg[8][0], 4);
    {
        uint64_t _a = 0, _b = 0, _v;
        memcpy(&_a, &base, 4); memcpy(&_b, &nominal, 4);
@OLD@
        memcpy(&was, &_v, 4);
    }
    CHECK(memcmp(&r, &was, 4) == 0 && c->vreg[8][1] == 0);   /* (the rule and the text patch: the same bits) */
    return r;
}
static float frames(int n, double seconds) {   /* (n presents that far apart: the seconds the hook is given, 0 when off) */
    float f;
    uint32_t bits;
    for (int i = 0; i < n; i++) now += (int64_t)(seconds * 1e9), realtime_step(now);
    bits = hos_frame_seconds_bits;
    memcpy(&f, &bits, 4);
    return bits ? f : 0.0f;
}
static int near(float a, double b) { return a > b * 0.999 && a < b * 1.001; }
int main(int argc, char **argv) {
    int *limit = (int *)GP(MOD + 0xfc), *rate = (int *)GP(MOD + 0x2c8);
    const char *mode = argc > 1 ? argv[1] : "";
    *limit = 60, *rate = 60;
    hos_step_probe = strcmp(mode, "no hook") ? 0x2000 : 0;   /* (the lifted code sets it when the hook is in the build) */
    realtime_step(now);
    if (!strcmp(mode, "clock log")) {
        CHECK(clock_logs == 0);
        frames(299, 1.0 / 60);
        CHECK(clock_logs == 0);
        frames(1, 1.0 / 60);
        CHECK(clock_logs == 1 && clock_now == now);
        frames(1, 1.0 / 60);
        CHECK(clock_logs == 1);
    } else if (!strncmp(mode, "cap 30", 6)) {
        CHECK(fps_max() == 30 && *limit == 30);
        CHECK(near(frames(600, 1.0 / 30), 1.0 / 30));
        CHECK(near(lifted_step(60.0f, (float)*rate), 2.0));
        live_cap = 0;
        if (!getenv("HOS_FPS_MAX")) CHECK(frames(10, 1.0 / 60) == 0.0f && *limit == 60);
    } else if (!strcmp(mode, "cap 120")) {
        CHECK(near(frames(72000, 1.0 / 120), 1.0 / 120) && *rate == 60 && *limit == 120);   /* (the rate stays the engine's) */
        CHECK(near(lifted_step(60.0f, (float)*rate), 0.5) && near(lifted_step(30.0f, (float)*rate), 0.25));   /* (half a 60th a frame) */
        CHECK(near(frames(10, 1.0 / 40), 1.0 / 40) && *rate == 40 && near(lifted_step(60.0f, (float)*rate), 1.5));
        CHECK(near(frames(10, 1.0 / 1000), 1.0 / 240));   /* (the shortest step) */
        CHECK(near(frames(10, 0.2), 4.0 / 60));           /* (the longest, as the rate's 15) */
    } else if (!strcmp(mode, "uncapped")) {   /* (frame_rate_cap=6, or HOS_FPS_MAX=1000: the frames' own time down to 1 ms) */
        CHECK(near(frames(10, 1.0 / 120), 1.0 / 120) && *rate == 60 && *limit == 1000);
        CHECK(near(frames(10, 1.0 / 500), 1.0 / 500));    /* (500 fps: a 500th of game time each, not a 240th) */
        CHECK(near(lifted_step(60.0f, (float)*rate), 0.12));   /* (60 * a 500th: real time) */
        CHECK(near(frames(10, 1.0 / 5000), 1.0 / 1000));  /* (the shortest step, the pacing's 1 ms) */
    } else if (!strcmp(mode, "live")) {   /* (started at 120, frame_rate_cap=2; the row changed in play) */
        CHECK(near(frames(600, 1.0 / 120), 1.0 / 120) && *rate == 60 && *limit == 120);
        live_cap = 0;   /* (60: the game's own limiter and step back) */
        CHECK(frames(10, 1.0 / 60) == 0.0f && *limit == 60 && *rate == 60);
        live_cap = 4;   /* (165) */
        CHECK(near(frames(10, 1.0 / 165), 1.0 / 165) && *limit == 165 && *rate == 60);
        live_cap = 3;   /* (144: the limiter comes down with it) */
        CHECK(near(frames(10, 1.0 / 144), 1.0 / 144) && *limit == 144);
        live_cap = 6;   /* (Uncapped) */
        CHECK(near(frames(10, 1.0 / 500), 1.0 / 500) && *limit == 1000);
        live_cap = 0;
        CHECK(frames(10, 1.0 / 60) == 0.0f && *limit == 60 && *rate == 60);
    } else if (!strcmp(mode, "live step off")) {   /* (HOS_FPS_STEP=0: the rate went up to 120; at 60 it must come back) */
        CHECK(frames(10, 1.0 / 120) == 0.0f && *rate == 120 && *limit == 120);
        live_cap = 0;
        CHECK(frames(10, 1.0 / 60) == 0.0f && *rate == 60 && *limit == 60);
    } else if (!strcmp(mode, "unknown index")) {   /* (frame_rate_cap= a later build's value: 60, as on the console) */
        CHECK(frames(10, 1.0 / 120) == 0.0f && *limit == 60);
    } else if (!strcmp(mode, "cap 60")) {
        CHECK(frames(72000, 1.0 / 120) == 0.0f && *rate == 60 && *limit == 60);
        /* A long unpaced stretch must repay only bounded debt, including the five-frame smoothing transition. */
        int recovered = 0;
        for (int i = 0; i < 24; i++) {
            CHECK(frames(1, 1.0 / 40) == 0.0f && *rate >= 15 && *rate <= 60);
            if (*rate == 40) { recovered = 1; break; }
        }
        CHECK(recovered && lifted_step(60.0f, (float)*rate) == 1.5f);
    } else if (!strcmp(mode, "movie pace")) {   /* (issue #73: at the console's cap a movie's player pacing goes to 240 while it updates, back to 60 after) */
        CHECK(frames(3, 1.0 / 60) == 0.0f && *limit == 60 && *rate == 60);   /* (no movie seen: nothing changes) */
        movie_seen_at = (uint64_t)now;   /* (the movie's player just updated: the lift's hook stamps this in the host) */
        CHECK(near(frames(10, 1.0 / 120), 1.0 / 120) && *limit == 240 && *rate == 60);   /* (paced and stepped: the frames' own time, the limiter raised) */
        CHECK(near(frames(10, 1.0 / 240), 1.0 / 240) && *limit == 240);   /* (kept while the window holds) */
        CHECK(frames(16, 1.0 / 60) == 0.0f && *limit == 60 && *rate == 60);   /* (the window passed with no new updates (16 x 1/60 = 266 ms > 250 ms): the game's limiter and step back) */
        movie_seen_at = (uint64_t)now;   /* (another movie: up again, and the first movie's game_limit is not lost) */
        CHECK(near(frames(5, 1.0 / 120), 1.0 / 120) && *limit == 240);
    } else if (!strcmp(mode, "movie cap30") || !strcmp(mode, "movie cap30 per frame")) {
        CHECK(frames(3, 1.0 / 60) == 0.0f && *limit == 60);
        movie_seen_at = (uint64_t)now;
        CHECK(near(frames(10, 1.0 / 240), 1.0 / 240) && *limit == 240);
        live_cap = -1;
        CHECK(hos_vi_native_interval(NULL, 2) == 2 && present_interval == 1);
        CHECK(near(frames(10, 1.0 / 30), 1.0 / 30) && *limit == 30);
        CHECK(near(lifted_step(60.0f, (float)*rate), 2.0));
        movie_seen_at = (uint64_t)now;
        live_cap = 0;
        CHECK(near(frames(10, 1.0 / 240), 1.0 / 240) && *limit == 240);
        CHECK(frames(16, 1.0 / 60) == 0.0f && *limit == 60 && *rate == 60);
        CHECK(hos_vi_native_interval(NULL, 2) == 2);
        CHECK(present_interval == (getenv("T_PER_FRAME") ? 2 : 1));
        CHECK(rx_option(0, 1) == (getenv("T_PER_FRAME") ? 0 : 1));
    } else if (!strcmp(mode, "movie pace off")) {   /* (HOS_MOVIE_PACE=0: even a fresh movie stamp leaves the game's cap) */
        movie_seen_at = (uint64_t)now;
        CHECK(frames(10, 1.0 / 60) == 0.0f && *limit == 60 && *rate == 60);
    } else {   /* (HOS_FPS_STEP=0, or a lift without the hook: the rate up to the cap, as before) */
        CHECK(frames(10, 1.0 / 120) == 0.0f && *rate == 120 && *limit == 120);
        CHECK(lifted_step(60.0f, 60.0f) == 1.0f);
    }
    printf("PASS: %s\n", mode);
    return 0;
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)

    sites = {x.name: x for x in load(BUILD)}
    values = host_values()
    for name, function, offset, out_reg in (("frame.stepDivide", "f_80706be0", 0xe4, "s8"),
                                            ("frame.globalStepDivide", "f_83b6ea90", 0x68, "s0")):
        x = sites.get(name)
        check(x is not None and (x.function, x.at, x.offset, x.rule, x.value) == (function, "after", offset, "mul_f32", "frame.seconds"),
              "%s is not the frame's seconds after %s's divide" % (name, function))
        check(set(x.args) == {out_reg + ":f32:out", "s1:f32:in"}, "%s does not put base * seconds in the divide's result" % name)
        check(x.signature[1][0] & 0xffe0fc00 == 0x1e201800 or x.signature[0][0] & 0xffe0fc00 == 0x1e201800,
              "%s's signature does not hold its divide" % name)
    probe = sites.get("frame.stepProbe")
    check(probe is not None and (probe.function, probe.at, probe.offset, probe.rule, probe.value) ==
          ("f_80706be0", "after", 0xe4, "note_u64", "frame.stepObject") and "x19:ptr:what" in probe.args,
          "the frame's step does not tell the host the hook is there")
    check(values.get("frame.seconds", (None, None, None))[1:] == ("uint32_t", "hos_frame_seconds_bits") and
          values.get("frame.stepObject", (None, None, None))[1:] == ("uint64_t", "hos_step_probe"),
          "game/sites.def does not give the rules host/nn_vi.c's values")
    step = sites["frame.stepDivide"]
    check(step.ident not in host_sites(), "the step's rule is not inline: sites.def gives it a callback slot")
    rule = sitehooks.line(step)

    # (the menu's row: Uncapped is its 6, after the six caps, so a saved 0 to 5 means what it did)
    menu = menu_text()   # (vk_menu.cpp and its modules' rows, #1633)
    check('"60 (the console\'s)", "90", "120", "144", "165", "240", "Uncapped"}' in menu, "tx_fps_cap does not end with Uncapped")
    row = menu[menu.index('{TAB_GRAPHICS, "Game frame rate cap"'):]
    row = row[:row.index("},") + 2]
    check("&opts.fps_cap, -1, 6, 1, tx_fps_cap," in row, "the Game frame rate cap row does not reach Uncapped")
    check("V-Sync" in row and "real time" in row, "the row's help does not say Uncapped keeps real time and V-Sync limits it")

    vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
    fps = vi[vi.index("enum { FPS_UNCAPPED"):]   # (the cap's values and fps_of, fps_refresh, fps_max)
    fps = fps[:fps.index(block(vi, "static int fps_max(void) {")) + len(block(vi, "static int fps_max(void) {"))]
    host = "\n".join([block(vi, "static float text_scale("), block(vi, "static float camera_scale("), fps] +
                     [block(vi, m) for m in ("int hos_vi_native_interval(", "static int realtime_rate(", "static void realtime_step(")])
    # (the option ids realtime_step names: vk_api.h's HOS_OPT_* enums)
    opts = "\n".join(re.findall(r"enum \{ HOS_OPT_[^}]*\};", (ROOT / "gpu" / "vk" / "vk_api.h").read_text(encoding="utf-8")))
    fixture = FIXTURE.replace("@OPTS@", opts).replace("@HOST@", host).replace("@HOOK@", rule).replace("@OLD@", OLD_HOOK)
    with tempfile.TemporaryDirectory(prefix="cap-step-") as tmp:
        c = Path(tmp) / "fixture.c"
        exe = Path(tmp) / "fixture.exe"
        c.write_text(fixture, encoding="utf-8")
        subprocess.run([a.cc, "-std=c11", "-include", ENV_ON_H, "-Wall", "-Wextra", "-Werror", str(c),
                        *sitehooks.objects(tmp, fixture), "-o", str(exe)], check=True)
        for mode, cap, step, index in (("cap 30 per frame", None, None, "-1"), ("cap 30", "30", None, None), ("cap 30", None, None, "-1"), ("cap 120", "120", None, None), ("cap 60", None, None, None), ("step off", "120", "0", None),
                                       ("no hook", "120", None, None), ("uncapped", None, None, "6"), ("uncapped", "1000", None, None),
                                       ("unknown index", None, None, "7"), ("cap 120", None, None, "2"), ("clock log", "60", None, None),
                                       ("live", None, None, "2"), ("live step off", None, "0", "2"),
                                       ("movie pace", None, None, None), ("movie cap30", None, None, None), ("movie cap30 per frame", None, None, None), ("movie pace off", None, None, None)):
            env = {k: v for k, v in os.environ.items() if not k.startswith("HOS_") and k != "T_CAP_INDEX"}
            if "per frame" in mode:
                env["T_PER_FRAME"] = "1"
            if mode == "clock log":
                env["HOS_FPS_LOG"] = "1"
            if mode == "movie pace off":
                env["HOS_MOVIE_PACE"] = "0"
            if cap:
                env["HOS_FPS_MAX"] = cap
            if step:
                env["HOS_FPS_STEP"] = step
            if index:
                env["T_CAP_INDEX"] = index   # (frame_rate_cap= in the options file)
            subprocess.run([str(exe), mode], env=env, check=True)


if __name__ == "__main__":
    main()
