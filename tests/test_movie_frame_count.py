"""Above a 60 cap a movie's frame count grows by the frames' time, so a cutscene's cues stay with its movie (issue 1742).

The game's movie player counts its updates while a movie plays, and a cutscene that shows a movie places its narration, its
rumble and its end by that count. The movie's picture and sound follow the clock, so at 120 fps the cues came twice too early:
narration lines started over each other. The hook site movie.frameCount (game/sites/sites.toml, a replace site on the
count's add) makes the count's step hos_movie_frames(player) (game/oot_sites.c), and host/nn_vi.c answers 1 while the host
gives no frame time (a cap of 60: as shipped) and the frames' time in 60ths, the rest carried to the player's next update,
while it does (HOS_FPS_MOVIE=0: 1 then too, for comparisons). The update is built with the hook as the lift lowers it (tests/sitehooks.py) and,
side by side, with the text patch it replaced (OLD, as main had it): both pass the same checks, and the game's own count fails
them. A game version with no site table keeps its own code.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_movie_frame_count.py [--cc gcc]
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]

# (env_on, the switches' rule, before the code cut out of the sources: gpu/vk/compat/longshot_env.h)
ENV_ON_H = str(Path(__file__).resolve().parents[1] / "gpu" / "vk" / "compat" / "longshot_env.h")
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import site, callback  # noqa: E402
import sitehooks  # noqa: E402

# (the lifted update's three instructions: the count loaded, one added, stored; OLD_ADD the text patch's add on main)
LOAD = "    c->x[8]=recomp_load32(c,c->x[20] + 760);\n"
ADD = "    { uint64_t _a=c->x[8], _b=1ULL; uint64_t _r=_a+_b; _r&=0xFFFFFFFFULL; c->x[8]=_r; }\n"
OLD_ADD = ADD.replace("_b=1ULL", "_b=hos_movie_frames(c->x[20])")
STORE = "    recomp_store32(c,c->x[20] + 760,c->x[8]);\n"
HOST_ANCHOR = "uint32_t hos_movie_frames(uint64_t player) {"


def block(source, marker):
    if marker not in source:
        raise SystemExit("FAIL: host/nn_vi.c has no '%s'" % marker)
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
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
/* (the lock as a flag: taken once, given back once) */
typedef int SRWLOCK;
#define SRWLOCK_INIT 0
static void AcquireSRWLockExclusive(SRWLOCK *l) { CHECK(!*l); *l = 1; }
static void ReleaseSRWLockExclusive(SRWLOCK *l) { CHECK(*l); *l = 0; }
volatile uint32_t hos_frame_seconds_bits;
/* (issue #73: the extracted hos_movie_frames stamps the movie's last update with the host clock) */
static volatile uint64_t movie_seen_at;   /* (the host's: the extracted code reads and writes it) */
static int64_t clock_ns = 1000000000LL;
static int64_t hos_now_ns(void) { return clock_ns; }
@HOST@
/* (the lifted update with the hook or the patch, on a small guest: the count at +760 of the player) */
#define A64_NOESC
typedef struct { uint64_t x[31]; } Ctx;
@CALLBACK@
static uint32_t guest[2][256];
static uint32_t recomp_load32(Ctx *c, uint64_t a) { (void)c; return guest[a >> 12][(a & 0xfff) / 4]; }
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { (void)c; guest[a >> 12][(a & 0xfff) / 4] = (uint32_t)v; }
static uint32_t update(int player) {
    Ctx ctx, *c = &ctx;
    memset(&ctx, 0, sizeof ctx);
    c->x[20] = (uint64_t)player << 12;
@LIFTED@
    return guest[player][760 / 4];
}
static void seconds(double s) {
    const float f = (float)s;
    uint32_t bits;
    memcpy(&bits, &f, 4);
    hos_frame_seconds_bits = s > 0 ? bits : 0;
}
static int within(uint32_t got, uint32_t want) { return got + 1 >= want && got <= want + 1; }
int main(int argc, char **argv) {
    uint32_t n = 0, before;
    if (argc > 1 && !strcmp(argv[1], "off")) {   /* (HOS_FPS_MOVIE=0: the game's own count at any rate) */
        seconds(1.0 / 120);
        for (int i = 0; i < 600; i++) n = update(0);
        CHECK(n == 600);
        printf("PASS: off\n");
        return 0;
    }
    /* no frame time (a cap of 60): one an update, as the game counts */
    seconds(0);
    for (int i = 0; i < 600; i++) n = update(0);
    CHECK(n == 600);
    CHECK(hos_movie_frames(0x5000) == 1 && hos_movie_frames(0x5000) == 1);
    /* (issue #73: the stamp happens before the frame time is read, so a movie player updating at the console's cap is
       seen by the pacing even though the count keeps the game's own step) */
    CHECK(movie_seen_at == (uint64_t)clock_ns);
    /* 120 fps: half a 60th an update, a whole one every second update */
    seconds(1.0 / 120);
    before = n;
    for (int i = 0; i < 600; i++) {
        const uint32_t was = n;
        n = update(0);
        CHECK(n - was <= 1);
    }
    CHECK(within(n - before, 300));
    /* 240 fps and 40 fps: a quarter and one and a half */
    seconds(1.0 / 240);
    before = n;
    for (int i = 0; i < 960; i++) n = update(0);
    CHECK(within(n - before, 240));
    seconds(1.0 / 40);
    before = n;
    for (int i = 0; i < 400; i++) {
        const uint32_t was = n;
        n = update(0);
        CHECK(n - was == 1 || n - was == 2);
    }
    CHECK(within(n - before, 600));
    /* two movies at once: each its own count (one carry for both would give each half) */
    seconds(1.0 / 120);
    before = n;
    {
        const uint32_t other = update(1) - 0;   /* (its first update) */
        uint32_t m = other;
        for (int i = 0; i < 600; i++) n = update(0), m = update(1);
        CHECK(within(n - before, 300) && within(m - other, 300));
    }
    /* a movie that plays while others come and go keeps its count: 14 more that play with it and a new player every update,
       16 at a time, at 240 fps (places taken in the order they were made gave this one a new start every second update: a
       quarter of a 60th an update then counted 0 or 1 every two, 1200 only on average) */
    seconds(1.0 / 240);
    {
        uint32_t mine = 0, steady = 0;
        for (int i = 0; i < 4800; i++) {
            mine += hos_movie_frames(0x7000000);
            for (int k = 0; k < 14; k++) steady += hos_movie_frames(0x7100000 + (uint64_t)k * 0x1000);
            hos_movie_frames(0x8000000 + (uint64_t)i * 0x1000);
        }
        CHECK(within(mine, 1200) && steady + 14 >= 14 * 1200 && steady <= 14 * 1200 + 14);
    }
    seconds(1.0 / 120);
    /* more movies at once than the host keeps places for, each updated in turn: every update finds its player's place taken and
       starts from a drawn part of a 60th, so 480 updates of half a 60th give 240 on average, 11 either way as one standard
       deviation. Every count within 5 of those, for any number of players: with a new place started at 0 every count stood
       still, and with a fixed step between the starts one player's did among 233 (and 29 players gave 108 to 131 of 120) */
    {
        static const int counts[] = {9, 17, 29, 40, 233, 377};
        static uint32_t sum[377];
        uint32_t least = ~0u, most = 0;
        for (unsigned t = 0; t < sizeof counts / sizeof counts[0]; t++) {
            const int players = counts[t];
            memset(sum, 0, sizeof sum);
            for (int i = 0; i < 480; i++)
                for (int k = 0; k < players; k++) sum[k] += hos_movie_frames(0x100000 + (uint64_t)t * 0x1000000 + (uint64_t)k * 0x1000);
            for (int k = 0; k < players; k++) {
                CHECK(sum[k] >= 240 - 55 && sum[k] <= 240 + 55);
                least = sum[k] < least ? sum[k] : least, most = sum[k] > most ? sum[k] : most;
            }
        }
        printf("more players than places: %u to %u of 240\n", least, most);
    }
    before = n;
    for (int i = 0; i < 600; i++) n = update(0);
    CHECK(within(n - before, 300));
    /* back at a cap of 60: one an update again */
    seconds(0);
    before = n;
    for (int i = 0; i < 10; i++) n = update(0);
    CHECK(n - before == 10);
    printf("PASS\n");
    return 0;
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)

    x = site("movie.frameCount", "f_83b7c710", "replace", 0xb8)
    check(("x20:ptr:player" in x.args) and ("w8:u32:count" in x.args), "the site does not name the player and the count")
    hooked = LOAD + sitehooks.line(x) + "\n" + ADD + sitehooks.REPLACE_END + "\n" + STORE
    slot = (callback("int hos_site_movie_frame_count(") + "\nvolatile uint8_t hos_site_on_movie_frameCount = 1;\n"
            "int (*volatile hos_site_fn_movie_frameCount)(Ctx *) = hos_site_movie_frame_count;\n")

    vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
    host = block(vi, HOST_ANCHOR)
    with tempfile.TemporaryDirectory(prefix="movie-count-") as tmp:
        c = Path(tmp) / "fixture.c"
        exe = Path(tmp) / "fixture.exe"
        env = {k: v for k, v in os.environ.items() if not k.startswith("HOS_")}
        # (the hook, its flag on; the hook with its flag off, the game's own add; the text patch it replaced)
        for what, callback_c, lifted, works in (("the hook", slot, hooked, True),
                                                ("the hook off", slot.replace("_frameCount = 1;", "_frameCount = 0;"), hooked, False),
                                                ("the text patch", "uint32_t hos_movie_frames(uint64_t player);\n", LOAD + OLD_ADD + STORE, True)):
            text = FIXTURE.replace("@HOST@", host).replace("@CALLBACK@", callback_c).replace("@LIFTED@", lifted)
            c.write_text(text, encoding="utf-8")
            subprocess.run([a.cc, "-std=c11", "-include", ENV_ON_H, "-Wall", "-Wextra", "-Werror", "-Wno-unused-function",
                            "-Wno-unused-variable", str(c), *sitehooks.objects(tmp, text), "-o", str(exe)], check=True)
            r = subprocess.run([str(exe)], env=env, capture_output=True, text=True)
            if not works:   # (the negative control: the game's own step of one an update fails the same checks at 120 fps)
                check(r.returncode != 0 and "FAIL:" in r.stderr, "%s passes the checks: they prove nothing" % what)
                continue
            check(r.returncode == 0, "%s: %s" % (what, r.stdout + r.stderr))
            subprocess.run([str(exe)], env=dict(env, HOS_FPS_MOVIE="1"), check=True)   # (read by its value: 1 is on)
            subprocess.run([str(exe), "off"], env=dict(env, HOS_FPS_MOVIE="0"), check=True)
            r = subprocess.run([str(exe), "off"], env=env, capture_output=True, text=True)
            check(r.returncode != 0 and "FAIL:" in r.stderr, "%s: the count is the game's own without HOS_FPS_MOVIE=0" % what)
    print("PASS: a movie's frame count steps by the frames' time through its hook site, as through the text patch")


if __name__ == "__main__":
    main()
