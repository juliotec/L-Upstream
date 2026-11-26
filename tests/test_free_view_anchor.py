"""The free camera's hook goes at the head of the view's loop in f_804e48c0: the hook site camera.view (game/sites/sites.toml),
before the instruction at +0x7a8, with game/oot_sites.c's callback hos_oot_free_view(c, x20).

A before site is placed by the instruction's address, on every way into it: the loop's entry and each time round, whatever
the lift writes there. The hook, as the lift lowers it (tests/sitehooks.py), with its callback runs against a stub
hos_oot_free_view: it is handed the context and x20, and the loop goes on.

python -I -B tests/test_free_view_anchor.py [--cc clang] [--cargo cargo]
"""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import CTX, check, hands, reg, site  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

STUBS = "void hos_oot_free_view(Ctx *c, uint64_t view) { calls++, got[0] = (uint64_t)(uintptr_t)c, got[1] = view; }\n"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    x = site("camera.view", "f_804e48c0", "before", 0x7a8)
    check("x20:ptr:view" in x.args, "the site does not name the view")
    check(not x.can_return, "the hook leaves the loop")
    hands(a.cc, [(x, "int hos_site_camera_view(", (CTX, reg(20)))], STUBS)
    print("PASS: the free camera's hook is a before site at the head of the view's loop")


if __name__ == "__main__":
    main()
