"""The Debug tab's Movement speed (1x to 3x): four hook sites (game/sites/sites.toml, game/oot_sites.c) hand the places a
character controller's forward speed is set to hos_oot_cct_speed with s0's unit, where the text patch they replaced
(nx2/oot_patches.py patch_cct_speed, deleted) put the same calls: Link's move f_809552f0 after it takes the controller for each
of its two setter calls (player.speedMove, player.speedMoveOther: a frame's speed), the setter f_80559df0's entry
(player.speedSet) and f_80964bf0 after its first load (player.speedSetComponent: a second's). Only the supported build has a
table. game/oot_tools.c's hos_oot_cct_speed scales the speed for
Link's controller alone, only while the left stick is pushed and only up to 15 m/s (a ledge climb's 55 is left alone); the menu
sends the row's percent (menu_link_speed), 100 while the debug tools are off. The four hooks, as the lift lowers them
(tests/sitehooks.py), with their callbacks run against a stub hos_oot_cct_speed: each hands it the context, x0 and its
unit, and the function goes on.

python -I -B tests/test_movement_speed.py [--cc clang] [--cargo cargo]
"""
import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import CTX, engine_fn, hands, reg, site  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]
STUBS = r"""
void hos_oot_cct_speed(Ctx *c, uint64_t controller, int per_second) {
    calls++, got[0] = (uint64_t)(uintptr_t)c, got[1] = controller, got[2] = 0x100 | (uint64_t)per_second;
}
"""

SITES = (("player.speedMove", "f_809552f0", "after", 0x134, 0), ("player.speedMoveOther", "f_809552f0", "after", 0x1ec, 0),
         ("player.speedSet", "f_80559df0", "entry", None, 1), ("player.speedSetComponent", "f_80964bf0", "after", 0x0, 1))


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    runs = []
    for name, function, at, offset, per_second in SITES:
        x = site(name, function, at, offset)
        check("x0:ptr:controller" in x.args and "s0:f32:speed" in x.args, "%s does not name the controller and the speed" % name)
        check(not x.can_return, "%s's hook leaves the function" % name)
        fn = engine_fn(x)
        check(fn in ("hos_site_speed_per_frame", "hos_site_speed_per_second"), "%s's slot names %s" % (name, fn))
        # (the unit the host is told is the site's: a frame's 0, a second's 1, whichever callback the slot names)
        runs.append((x, "int %s(" % fn, (CTX, reg(0), 0x100 | per_second)))
    hands(a.cc, runs, STUBS)

    tools = (ROOT / "game" / "oot_tools.c").read_text(encoding="utf-8")
    fn = tools[tools.index("void hos_oot_cct_speed("):]
    fn = fn[:fn.index("\n}\n")]
    check("oot_speed_controller" in fn and "hos_move_stick_held()" in fn, "the host scales only Link's speed while he is steered")
    check(re.search(r"<= 15\.0f", fn) is not None, "the host leaves speeds over 15 m/s alone")
    menu = (ROOT / "gpu" / "vk" / "vk_menu.cpp").read_text(encoding="utf-8")
    check("constexpr int link_speeds[] = {100, 125, 150, 200, 300};" in menu, "the row's choices are 1x, 1.25x, 1.5x, 2x, 3x")
    check(re.search(r"int menu_link_speed\(\) \{ return cheats_on\(\) \? .* : 100; \}", menu) is not None,
          "the row sends 100 while the debug tools are off")
    print("PASS: movement speed hooks, host gate and menu choices")


if __name__ == "__main__":
    main()
