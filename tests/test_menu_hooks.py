"""The game's menus' mouse (game/oot_menu.c) and the button prompts changed while the game runs (host/prompts.c) hook five
functions of the lifted game at their entries, hook sites of the supported build's table (game/sites/sites.toml, with
game/oot_sites.c's callbacks), as the text patches they replaced put each call first:

- menu.focus, a menu screen's handler of a pane getting the focus (f_802ed660): hos_oot_menu_focus with the screen, the pane
  and the caller's return address;
- menu.update, a menu screen's update (f_802f1380): hos_oot_menu_tick with the screen;
- text.boxDraw, a text box's draw (f_8152b670): hos_prompts_text_draw with the box;
- text.boxLayout, its layout (f_8152aec0): hos_prompts_text_layout with the box;
- font.glyphRender, FreeType's ft_smooth_render (f_81680c60): hos_prompts_glyph with the glyph slot (x1).

Each hook goes on into the function (it only looks); menu.update's function is also the gameplay HUD's wrapper
(builder/crates/edits/src/patches.rs, wrappers). Only the supported build has a table, so another build's lift gets none of them, as the patches were the supported build's only.

The hooks, as the lift lowers them (tests/sitehooks.py), with their callbacks (the ones game/sites.def names for
them) run against stubs of the host's functions, each told which it is (its first value), over known registers: each is
handed what is listed above and the function goes on.

python -I -B tests/test_menu_hooks.py [--cc clang] [--cargo cargo]
"""
import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import CTX, check, engine_fn, hands, reg, site  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

# (each site, its function, its callback (game/sites.def's), and what the host is handed: which stub (STUBS), then
# its arguments)
SITES = (
    ("menu.focus", "f_802ed660", "hos_site_menu_focus", (1, CTX, reg(0), reg(1), reg(30))),
    ("menu.update", "f_802f1380", "hos_site_menu_update", (2, CTX, reg(0))),
    ("text.boxDraw", "f_8152b670", "hos_site_text_box_draw", (3, reg(0))),
    ("text.boxLayout", "f_8152aec0", "hos_site_text_box_layout", (4, reg(0))),
    ("font.glyphRender", "f_81680c60", "hos_site_glyph_render", (5, reg(1))),
)
STUBS = r"""
void hos_oot_menu_focus(Ctx *c, uint64_t screen, uint64_t pane, uint64_t lr) {
    calls++, got[0] = 1, got[1] = (uint64_t)(uintptr_t)c, got[2] = screen, got[3] = pane, got[4] = lr;
}
void hos_oot_menu_tick(Ctx *c, uint64_t screen) { calls++, got[0] = 2, got[1] = (uint64_t)(uintptr_t)c, got[2] = screen; }
void hos_prompts_text_draw(uint64_t box) { calls++, got[0] = 3, got[1] = box; }
void hos_prompts_text_layout(uint64_t box) { calls++, got[0] = 4, got[1] = box; }
void hos_prompts_glyph(uint64_t slot) { calls++, got[0] = 5, got[1] = slot; }
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    runs = []
    for name, function, fn, handed in SITES:
        x = site(name, function)
        check(not x.can_return, "%s's hook returns from the function" % name)
        check(engine_fn(x) == fn, "%s's slot does not name %s" % (name, fn))
        runs.append((x, "int %s(" % fn, handed))
    hands(a.cc, runs, STUBS)
    patches = (Path(__file__).resolve().parents[1] / "builder" / "crates" / "edits" / "src" / "patches.rs").read_text(encoding="utf-8")
    check('(0x802f_1380, Wrap::Host { host: "hos_gameplay_hud_screen".into() })' in patches,
          "builder/crates/edits/src/patches.rs no longer wraps menu.update's function in the gameplay HUD's hos_gameplay_hud_screen")
    print("PASS: the menus', the text boxes' (draw, layout) and the glyphs' hooks are entry sites calling the host as the patches did")


if __name__ == "__main__":
    main()
