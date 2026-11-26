"""Widescreen's hook sites for the checks that compile them (tests/test_wide_sites.py, test_hud_layout.py, test_wide_ui.py,
test_oot_wide_bubble.py, test_oot_wide_save_picture.py, test_oot_camera_clearance.py). Standard library, tests/sitecheck.py
and tests/sitehooks.py only.

    sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
    from widesites import hook

hook(name, *helpers): (the C to put at file scope: game/oot_wide_sites.c's preamble (the build's addresses, its constants and
declarations), the static helpers named, the site's callback and its flag (on) and slot; the site's hook as the lift lowers
it at its instruction (tests/sitehooks.py), to put where the instruction's C ends; the fixture links
sitehooks.objects(tmp, its C)). A missing site or callback stops with SystemExit("FAIL: ...").
"""
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sitehooks  # noqa: E402
from sitecheck import BUILD, ROOT, check, engine_fn, host_sites, load, slot  # noqa: E402
from srccut import block  # noqa: E402

SOURCE = ROOT / "game" / "oot_wide_sites.c"


def preamble():
    text = SOURCE.read_text(encoding="utf-8")
    start, end = text.index("#include <string.h>\n"), text.index("/* A size just made")
    return re.sub(r"^#include.*$", "", text[start:end], flags=re.M)


def site(name):
    x = {s.name: s for s in load(BUILD)}.get(name)
    check(x is not None and x.at == "after", "the table has no after site %s" % name)
    check(host_sites().get(x.ident) == name, "game/sites.def has no HOS_SITE line for %s" % name)
    return x


def hook(name, *helpers):
    x = site(name)
    text = SOURCE.read_text(encoding="utf-8")
    fn = engine_fn(x)
    defs = preamble() + "".join(block(text, "static %s(" % h) + "\n" for h in helpers)
    defs += block(text, "int %s(" % fn) + "\n" + slot(x)
    return defs, sitehooks.line(x)
