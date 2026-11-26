"""The Insert menu's Camera speed scales the right stick's camera turn speeds, and only when the player asks for it.

The returns of the turn-speed functions (f_80643f10, f_80643b60 and their level-2 pair f_82ea21f0, f_82ea2220) are hook
sites (game/sites/sites.toml, at = "return"), whose callback, game/oot_sites.c's hos_site_camera_turn_speed, multiplies
s0 by hos_camera_scale, which host/nn_vi.c sets from the row (HOS_OPT_CAMERA_SPEED) at each present. At 100% the register
stays as the game left it. The callback and the text patch it replaced (kept below as main had it) leave the same
registers: the callback through each site's hook as the lift lowers it (tests/sitehooks.py), its flag on (off, the
register is the game's).

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_camera_speed.py [--cc clang] [--cargo cargo]
"""
import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from menu_model_fixture import menu_text  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
import sitecheck  # noqa: E402
import sitehooks  # noqa: E402
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (tests/srccut.py: python -I leaves this folder off the path)
from srccut import block   # noqa: E402

BUILD = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348"
SITES = (("camera.yawSpeed", "f_80643f10"), ("camera.pitchSpeed", "f_80643b60"),
         ("camera.yawSpeedLevel2", "f_82ea21f0"), ("camera.pitchSpeedLevel2", "f_82ea2220"))
# (the text patch's C on main before the sites replaced it: put in place of each "return;" of the four functions)
OLD_RETURN = ("{ if (hos_camera_scale != 1.0f) { float _t; uint32_t _u; memcpy(&_t,c->vreg[0],4); _t *= hos_camera_scale; "
              "memcpy(&_u,&_t,4); a64_vset64(c->vreg[0],_u); } return; }   /* (nx2/oot_patches.py: camera speed) */")


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def site_checks():
    """the four functions' returns are hook sites with the engine's callback"""
    sites = {x.name: x for x in sitecheck.load(BUILD)}
    for name, function in SITES:
        x = sites.get(name)
        check(x is not None and (x.at, x.function) == ("return", function), "%s is not the return of %s" % (name, function))
        check("s0:f32:speed" in x.args, "%s does not name the speed it scales" % name)
        check(sitecheck.host_sites().get(sitecheck.ident(name)) == name, "game/sites.def has no slot for " + name)
        check(sitecheck.engine_fn(x) == "hos_site_camera_turn_speed", "%s does not call hos_site_camera_turn_speed" % name)


def source_checks():
    vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
    check("hos_camera_scale = camera_scale(rx_option(HOS_OPT_CAMERA_SPEED, 100));" in block(vi, "static void realtime_step("),
          "the scale is not set from the menu's row at each present (100% without the renderer's menu)")
    replay = (ROOT / "gpu" / "vk" / "vk_replay.cpp").read_text(encoding="utf-8")
    api = (ROOT / "gpu" / "vk" / "vk_api.h").read_text(encoding="utf-8")
    api += (ROOT / "game" / "options.h").read_text(encoding="utf-8")   # (the game's ids: one space with vk_api.h's)
    value = {}   # (every HOS_OPT_* enum)
    for first, start, rest in re.findall(r"enum \{ (HOS_OPT_\w+) = (\d+)(?:, ([^}]*))? \}", api):
        at = int(start) - 1
        for n in [first + " = " + start] + [n.strip() for n in (rest or "").split(",") if n.strip()]:
            name, _, given = n.partition("=")   # (a later name's own value: game/options.h's HOS_OPT_SKIP_TITLE = 44)
            at = int(given) if given.strip() else at + 1
            value[name.strip()] = at
    check("HOS_OPT_CAMERA_SPEED" in value, "gpu/vk/vk_api.h has no HOS_OPT_CAMERA_SPEED")
    table = block(replay, "t.option = [](int id) -> int {")
    ids = [int(n) if n.isdigit() else value[n] for n in re.findall(r"\bid == (\w+)", table)]
    check(len(ids) == len(set(ids)), "vk_replay.cpp: an option id is read twice: %s" % sorted(ids))
    check("if (id == HOS_OPT_CAMERA_SPEED) return opts.camera_speed;" in table, "the host cannot see the menu's row")
    users = []
    for folder in ("host", "gpu", "game"):
        for path in (ROOT / folder).rglob("*.c*"):
            text = path.read_text(encoding="utf-8", errors="replace")
            if re.search(r"\brx_option(_set)?\(\s*(HOS_OPT_CAMERA_SPEED|%d)\s*," % value["HOS_OPT_CAMERA_SPEED"], text):
                users.append(path.relative_to(ROOT).as_posix())
    check(users == ["host/nn_vi.c"], "the Camera speed option is read or set outside host/nn_vi.c: %s" % users)

    header = (ROOT / "gpu" / "vk" / "vk_menu.h").read_text(encoding="utf-8")
    menu = menu_text()   # (vk_menu.cpp and its modules' rows, #1633)
    check("int camera_speed = 100;" in header, "out of the box the camera does not turn at the game's own speed")
    row = re.search(r'\{TAB_CONTROLS, "Camera speed", &opts\.camera_speed, 50, 400, 25, tx_percent,', menu)
    check(row is not None, "the Controls tab has no Camera speed row (50 to 400%, steps of 25)")
    check(block(menu, '{TAB_CONTROLS, "Camera speed"').rstrip().endswith("nullptr, false, true}"),
          "the Camera speed row takes a restart, or changes the Quality preset")
    # (a scale: a module's slider (gpu/vk/menu/camera.cpp, #1633), which is_slider and stops_at_ends take by its kind)
    camera = (ROOT / "gpu" / "vk" / "menu" / "camera.cpp").read_text(encoding="utf-8")
    check(re.search(r'\.key = "camera_speed",\s*\.label = "Camera speed",[^}]*\.kind = menu::Kind::Slider,', camera),
          "the Camera speed row is not a slider: a scale")
    for fn in ("bool is_slider(", "bool stops_at_ends("):
        line = menu[menu.index(fn):menu.index("\n", menu.index(fn))]
        check("menu::Kind::Slider" in line, "%s does not take a module's slider: a scale" % fn.split("(")[0])
    # (its line in longshot_options.ini: the row's own key, camera_speed, and no other row that key)
    ini_key = block(menu, "const char* ini_key(const Row& r) {")
    check('"camera_speed"' not in ini_key and menu.count('.key = "camera_speed"') == 1, "the row is not saved as camera_speed")
    settings = (ROOT / "docs" / "user" / "graphics-and-controls.md").read_text(encoding="utf-8")
    check("| | Camera speed | 50% to 400%, in steps of 25 | 100% |" in settings,
          "docs/user/graphics-and-controls.md's settings table has no Camera speed row")


def fixture_checks(cc):
    vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
    sites = [sitecheck.site(name, function, "return") for name, function in SITES]
    callback = sitecheck.callback("int hos_site_camera_turn_speed(") + "\n" + "".join(sitecheck.slot(x) for x in sites)
    # (each site's return: the function returned, then its wrapper runs the site's hook, as the lift lowers it)
    returns = "".join("static void new_return_%d(Ctx *__restrict c) {\n%s\n}\n" % (i, sitehooks.line(x))
                      for i, x in enumerate(sites))
    table = "static void (*const new_returns[])(Ctx *__restrict) = {%s};\n" % ", ".join(
        "new_return_%d" % i for i in range(len(sites)))
    table += "static volatile uint8_t *const flags[] = {%s};\n" % ", ".join("&hos_site_on_" + x.ident for x in sites)
    fixture = r'''#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; } Ctx;
static inline void a64_vset64(uint64_t *d, uint64_t v) { d[0] = v; d[1] = 0; }
volatile float hos_camera_scale = 1.0f;
''' + block(vi, "static float camera_scale(") + "\n" + callback + r'''
/* (a turn-speed function's return as the text patch made it, on main) */
static void old_return(Ctx *__restrict c) ''' + OLD_RETURN + r'''
''' + returns + table + r'''
static float turn(Ctx *c, float speed) {
    uint32_t u;
    Ctx was, start;
    memcpy(&u, &speed, 4);
    c->vreg[0][0] = 0xabcd000000000000ull | u, c->vreg[0][1] = 7;
    start = *c;
    for (unsigned i = 0; i < sizeof new_returns / sizeof *new_returns; i++) {
        *flags[i] = 0;   /* (off: the register as the game left it) */
        *c = start;
        new_returns[i](c);
        CHECK(memcmp(&start, c, sizeof start) == 0);
        *flags[i] = 1;
        *c = was = start;
        old_return(&was);
        new_returns[i](c);
        CHECK(memcmp(&was, c, sizeof was) == 0);   /* (the text patch and the site leave the same registers) */
    }
    float r;
    memcpy(&r, c->vreg[0], 4);
    return r;
}
int main(void) {
    Ctx c;
    memset(&c, 0, sizeof c);
    CHECK(camera_scale(100) == 1.0f && camera_scale(50) == 0.5f && camera_scale(250) == 2.5f && camera_scale(400) == 4.0f);
    CHECK(camera_scale(0) == 1.0f && camera_scale(49) == 1.0f && camera_scale(401) == 1.0f);   /* (not the row's: the game's) */
    hos_camera_scale = camera_scale(100);
    CHECK(turn(&c, 0.75f) == 0.75f && c.vreg[0][1] == 7 && c.vreg[0][0] >> 48 == 0xabcd);   /* 100%: the register untouched */
    hos_camera_scale = camera_scale(200);
    CHECK(turn(&c, 0.75f) == 1.5f && c.vreg[0][1] == 0 && c.vreg[0][0] >> 32 == 0);   /* (a write of s0 clears the rest of v0) */
    hos_camera_scale = camera_scale(50);
    CHECK(turn(&c, 0.75f) == 0.375f);
    printf("PASS: camera speed only scales the turn speeds when asked\n");
    return 0;
}
'''
    print(sitecheck.run(cc, fixture, "camera-speed-"), end="")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    site_checks()
    source_checks()
    fixture_checks(a.cc)


if __name__ == "__main__":
    main()
