"""The Insert menu's Text speed scales the letters' clock of a text pane, and only when the player asks for it; and the hook
site that does it does exactly what the text patch it replaced did.

Text speed and Text size were a text patch after the letter count's test at the head of f_80b1e140's letters' block. They
are the hook site text.letterStep now (game/sites/sites.toml),
placed after the instruction that loads the count, with game/oot_text_step.c's hos_text_letter_step as the engine's
callback. Checked here, game-free:

  - the site is in the supported build's table, after f_80b1e140+0x74, with the arguments the callback reads, and the host
    gives it its slot with that callback (game/sites.def);
  - the site's hook as the lift lowers it (tests/sitehooks.py), with the callback, and the patch's C, each where it ran (the patch after the count's test,
    the hook before it), leave the whole guest context and the Text size calls the same, for every count, scale and time
    tried, with other bits in the rest of v0: the patch's C is kept below as main had it;
  - the menu reaches hos_text_scale as before.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_text_speed.py [--cc gcc]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from menu_model_fixture import GAME_OPTION_ANSWER, game_row, menu_text  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (tests/srccut.py: python -I leaves this folder off the path)
import sitehooks  # noqa: E402
from sitecheck import host_sites, load  # noqa: E402
from srccut import block   # noqa: E402

BUILD = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348"

# (the text patch's C on main before the site replaced it, which followed "if ((uint32_t)c->x[9] == 0) { goto L_80b1e18c; }")
OLD_PATCH = ("    if (hos_text_scale != 1.0f) { float _t; uint32_t _u; memcpy(&_t,c->vreg[0],4); _t *= hos_text_scale; "
             "memcpy(&_u,&_t,4); a64_vset64(c->vreg[0],_u); }   /* (nx2/oot_patches.py: text speed) */\n"
             "    hos_oot_text_pane(c, c->x[19]);   /* (nx2/oot_patches.py: text size; x19 the LetterAnimControl) */\n")


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)

    sites = {s.name: s for s in load(BUILD)}
    site = sites.get("text.letterStep")
    check(site is not None, "the supported build's table has no text.letterStep")
    check((site.kind, site.function, site.at, site.offset) == ("code", "f_80b1e140", "after", 0x74),
          "text.letterStep is not after f_80b1e140+0x74 (the letter count's load)")
    check({"x19:ptr:anim", "w9:u32:letters", "s0:f32:time"} <= set(site.args), "the site does not name what the callback reads")
    check(site.shape == "data" and site.thread == "worker", "text.letterStep is not a data rule reachable off the serial thread")
    check(host_sites().get("text_letterStep") == "text.letterStep", "game/sites.def has no slot for text.letterStep")
    sites_def = (ROOT / "game" / "sites.def").read_text(encoding="utf-8")
    check('HOS_SITE_ENGINE(text_letterStep, "text.letterStep", hos_text_letter_step)' in sites_def,
          "the engine does not subscribe hos_text_letter_step to the site")

    vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
    replay = (ROOT / "gpu" / "vk" / "vk_replay.cpp").read_text(encoding="utf-8")
    menu = menu_text()   # (vk_menu.cpp and its modules' rows, #1633)
    option = replay[replay.index("t.option = "):]
    option = option[:option.index("};") + 2]
    options_h = (ROOT / "game" / "options.h").read_text(encoding="utf-8")
    check("enum { HOS_OPT_TEXT_SPEED = 5 };" in options_h and "rx_option(5, 0)" in vi, "the menu's row is not option 5")
    check(GAME_OPTION_ANSWER in option and "id == 5 ?" not in option, "the host cannot see the menu's row (option 5)")
    row = game_row("text_speed")   # (the game's row, game/menu/rows.c)
    check(row is not None and (row["label"], row["option"], row["lo"], row["hi"], row["step"]) ==
          ('"Text speed"', "HOS_OPT_TEXT_SPEED", "0", "3", "1"), "the Insert menu has no row for it")
    check("Instant" in menu, "the Text speed row's choices are gone")
    check("hos_text_scale = text_scale(" in block(vi, "static void realtime_step("), "the scale is not set each present")

    step = (ROOT / "game" / "oot_text_step.c").read_text(encoding="utf-8")
    fixture = r'''#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; uint8_t n, z, c, v; } Ctx;
static uint64_t pane_seen[8];   /* (the Text size calls: their x19, in order) */
static int panes;
static void hos_oot_text_pane(Ctx *c, uint64_t anim) { (void)c; if (panes < 8) pane_seen[panes] = anim; panes++; }
static inline void a64_vset64(uint64_t *d, uint64_t v) { d[0] = v; d[1] = 0; }
volatile float hos_text_scale = 1.0f;
''' + block(vi, "static float text_scale(") + "\n" + block(step, "int hos_text_letter_step(") + r'''
/* the site's flag and slot as game/sites.c gives them (HOS_SITE_ENGINE: on from the start) */
volatile uint8_t hos_site_on_text_letterStep = 1;
int (*volatile hos_site_fn_text_letterStep)(Ctx *A64_NOESC) = hos_text_letter_step;
/* main's f_80b1e140 at the letters' block, with the text patch: the count loaded, its test, then the patch's C */
static void old_block(Ctx *c, uint16_t letters) {
    c->x[9] = letters;
    if ((uint32_t)c->x[9] == 0) { goto L_return; }
''' + OLD_PATCH + r'''L_return:;
}
/* the same with the site: the count loaded, then the site's hook after that instruction, then the count's test */
static void new_block(Ctx *c, uint16_t letters) {
    c->x[9] = letters;
''' + sitehooks.line(site) + r'''
    if ((uint32_t)c->x[9] == 0) { goto L_return; }
L_return:;
}
static void start(Ctx *c, float dt, unsigned seed) {
    uint32_t u;
    memcpy(&u, &dt, 4);
    memset(c, 0, sizeof *c);
    for (unsigned i = 0; i < 32; i++) c->x[i] = 0x9e3779b97f4a7c15ull * (i + seed), c->vreg[i][1] = 0x1234567ull * (i + 1);
    c->vreg[0][0] = 0xabcd000000000000ull | u, c->vreg[0][1] = 7;
    c->x[19] = 0x10000 + seed;
}
static float s0(const Ctx *c) { float r; memcpy(&r, c->vreg[0], 4); return r; }
int main(void) {
    static const float scales[] = {1.0f, 2.0f, 4.0f, 1e5f, 0.5f, 1.0000001f};
    static const float times[] = {0.0f, 0.5f, 1.0f / 60, -0.25f, 3.0e38f};
    static const uint16_t counts[] = {0, 1, 7, 0xffff};
    unsigned seed = 0;
    for (unsigned k = 0; k < sizeof scales / sizeof scales[0]; k++)
        for (unsigned t = 0; t < sizeof times / sizeof times[0]; t++)
            for (unsigned n = 0; n < sizeof counts / sizeof counts[0]; n++, seed++) {
                Ctx was, now;
                uint64_t seen_was[8], seen_now[8];
                int panes_was, panes_now;
                hos_text_scale = scales[k];
                start(&was, times[t], seed), panes = 0, memset(pane_seen, 0, sizeof pane_seen);
                old_block(&was, counts[n]);
                panes_was = panes, memcpy(seen_was, pane_seen, sizeof seen_was);
                start(&now, times[t], seed), panes = 0, memset(pane_seen, 0, sizeof pane_seen);
                new_block(&now, counts[n]);
                panes_now = panes, memcpy(seen_now, pane_seen, sizeof seen_now);
                CHECK(memcmp(&was, &now, sizeof was) == 0);   /* every register, flag and vector register the same */
                CHECK(panes_was == panes_now && memcmp(seen_was, seen_now, sizeof seen_was) == 0);
                CHECK(panes_now == (counts[n] != 0));   /* (Text size: once a step with letters, never without) */
            }
    Ctx c;
    CHECK(text_scale(NULL, 0) == 1.0f && text_scale(NULL, 1) == 2.0f && text_scale(NULL, 2) == 4.0f);
    CHECK(text_scale(NULL, 3) >= 1e5f);           /* Instant: a page in one step */
    CHECK(text_scale("0", 3) == 1.0f);            /* HOS_TEXT_SPEED wins over the menu */
    CHECK(text_scale("", 1) == 2.0f);
    hos_text_scale = text_scale(NULL, 0);
    start(&c, 0.5f, 1), new_block(&c, 3);
    CHECK(s0(&c) == 0.5f && c.vreg[0][1] == 7 && c.vreg[0][0] >> 48 == 0xabcd);   /* Normal: the register untouched */
    hos_text_scale = text_scale(NULL, 2);
    start(&c, 0.5f, 1), panes = 0, new_block(&c, 3);
    CHECK(s0(&c) == 2.0f && c.vreg[0][1] == 0 && c.vreg[0][0] >> 32 == 0);
    CHECK(panes == 1 && pane_seen[0] == c.x[19]);   /* the text size hook gets x19 */
    start(&c, 0.5f, 1), panes = 0, new_block(&c, 0);
    CHECK(s0(&c) == 0.5f && panes == 0);   /* no letters: nothing */
    printf("PASS: the text.letterStep site does what the text patch did; text speed only scales when asked\n");
    return 0;
}
'''
    with tempfile.TemporaryDirectory(prefix="text-speed-") as tmp:
        c = Path(tmp) / "fixture.c"
        exe = Path(tmp) / "fixture"
        c.write_text(fixture, encoding="utf-8")
        subprocess.run([a.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(c), *sitehooks.objects(tmp, fixture), "-o", str(exe)],
                       check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == "__main__":
    main()
