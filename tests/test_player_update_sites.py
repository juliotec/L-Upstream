"""Link's update and the Debug tab's flight reach the host through three hook sites (game/sites/sites.toml, with
game/oot_sites.c's callbacks over game/oot_tools.c), where the text patches they replaced (nx2/oot_patches.py patch_movement and
patch_flight_rotation, deleted) put the same calls:

- player.update, before f_82c415e0's test of the actor's flag byte (tbnz w8, #1 at +0x14c): hos_oot_lift_player(c, x19);
- player.pose, before +0x198, where the update's two paths to the pose matrix meet (the patch's L_82c41778):
  hos_oot_flight_pose(c, x19, x25);
- player.flightRotation, before f_806d2530's call of the controller's virtual method (blr x8 at +0x94):
  hos_oot_flight_rotation(c, x0, x1).

A before site runs after the instruction's label, on every way into it, as the patch's text after the label did; the hook
goes on into the instruction. The hooks, as the lift lowers them (tests/sitehooks.py), with their callbacks run against
stubs of the three calls over known registers: each is handed what is listed above. The calls' own behaviour is checked
by the debug tools' and the state tap's checks.

python -I -B tests/test_player_update_sites.py [--cc clang] [--cargo cargo]
"""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import CTX, check, engine_fn, hands, reg, site  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

# (each site, its function and offset, its callback, the argument it names, and what the host is handed: which stub
# (STUBS), then its arguments)
SITES = (
    ("player.update", "f_82c415e0", 0x14c, "hos_site_player_update", "x19:ptr:actor", (1, CTX, reg(19))),
    ("player.pose", "f_82c415e0", 0x198, "hos_site_player_pose", "x25:ptr:matrix", (2, CTX, reg(19), reg(25))),
    ("player.flightRotation", "f_806d2530", 0x94, "hos_site_flight_rotation", "x1:ptr:matrix", (3, CTX, reg(0), reg(1))),
)
STUBS = r"""
void hos_oot_lift_player(Ctx *c, uint64_t actor) { calls++, got[0] = 1, got[1] = (uint64_t)(uintptr_t)c, got[2] = actor; }
void hos_oot_flight_pose(Ctx *c, uint64_t actor, uint64_t matrix) {
    calls++, got[0] = 2, got[1] = (uint64_t)(uintptr_t)c, got[2] = actor, got[3] = matrix;
}
void hos_oot_flight_rotation(Ctx *c, uint64_t controller, uint64_t matrix) {
    calls++, got[0] = 3, got[1] = (uint64_t)(uintptr_t)c, got[2] = controller, got[3] = matrix;
}
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    runs = []
    for name, function, offset, fn, arg, handed in SITES:
        x = site(name, function, "before", offset)
        check(arg in x.args, "%s does not name %s" % (name, arg))
        check(not x.can_return, "%s's hook leaves the function" % name)
        check(engine_fn(x) == fn, "%s's slot does not name %s" % (name, fn))
        runs.append((x, "int %s(" % fn, handed))
    hands(a.cc, runs, STUBS)
    print("PASS: Link's update, his pose and the flight's rotation reach the host through before sites, as the patches did")


if __name__ == "__main__":
    main()
