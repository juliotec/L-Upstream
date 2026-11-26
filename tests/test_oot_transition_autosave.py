"""Run the OoT save-request hook against synthetic requests, without game data: the hook site save.request (the entry of
f_80bc72d0, which can return: game/sites/sites.toml), as the lift lowers it (tests/sitehooks.py), with its callback
(game/oot_sites.c) and game/oot_tools.c's hos_oot_trace_save. A request Transition autosave drops returns from the
function at once with x0 = 0; every other goes on into it, x0 as it was; with the site off, every request goes on.

python -I -B tests/test_oot_transition_autosave.py [--cc clang] [--cargo cargo]
"""
import argparse
import sys
from pathlib import Path
import re

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves this folder off the path)
import sitecheck  # noqa: E402
import sitehooks  # noqa: E402

FIXTURE = r'''
#include <assert.h>
#include <stdint.h>
#include <stdio.h>

typedef struct { uint64_t x[32]; } Ctx;
static uint64_t parent;
static int autosave;

static uint64_t recomp_load64(Ctx *c, uint64_t address) {
    assert(address == c->x[31] + 24);
    return parent;
}

static int env_on(const char *name) { (void)name; return 0; }
#define hos_log(...) ((void)0)

int rx_option(int option, int fallback) {
    assert(option == HOS_OPT_TRANSITION_AUTOSAVE && fallback == 1);
    return autosave;
}

// HOOK

static int went_on;
static void save_request(Ctx *c) {
// SITE
    went_on = 1;
}

static void check(uint64_t caller, uint64_t site, unsigned reason, unsigned mode, int blocked) {
    for (int on = 0; on <= 1; on++) {
        Ctx c = {0};
        c.x[0] = 123;
        c.x[1] = reason;
        c.x[2] = mode;
        c.x[30] = caller;
        parent = site;
        went_on = 0;
        hos_site_on_save_request = (uint8_t)on;
        save_request(&c);
        assert(went_on == !(on && blocked));
        assert(c.x[0] == (on && blocked ? 0 : 123));
    }
}

int main(void) {
    const uint64_t wrapper = 0x80bc7764ULL;
    for (autosave = 0; autosave <= 1; ++autosave) {
        check(wrapper, 0x80bc7710ULL, 6, 0, !autosave);
        check(wrapper, 0x80bc7864ULL, 2, 0, !autosave);
        check(wrapper, 0x832a9764ULL, 1, 0, !autosave);
        check(wrapper, 0x80bc7864ULL, 1, 0, 0);
        check(wrapper, 0x80bc7864ULL, 6, 0, 0);
        check(wrapper, 0x80bc7710ULL, 2, 0, 0);
        check(wrapper, 0x80bc7864ULL, 2, 1, 0);
        check(0x83296490ULL, 0x80bc7864ULL, 2, 0, 0);
    }
    puts("ok");
    return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    tools = (ROOT / "game/oot_tools.c").read_text(encoding="utf-8")
    api = (ROOT / "game/options.h").read_text(encoding="utf-8")   # (the game's option ids)
    hook = re.search(r"^int hos_oot_trace_save\(Ctx \*c\) \{.*?^\}", tools, re.M | re.S).group()
    option = re.search(r"enum \{ HOS_OPT_TRANSITION_AUTOSAVE = \d+ \};", api).group()
    x = sitecheck.site("save.request", "f_80bc72d0", "entry", can_return=True)
    sitecheck.check(sitecheck.engine_fn(x) == "hos_site_save_request", "save.request's slot does not name hos_site_save_request")
    defs, line = sitecheck.hooked(x, "int hos_site_save_request(")
    code = option + "\n" + FIXTURE.replace("// HOOK", hook + "\n" + defs).replace("// SITE", line)
    sitecheck.check(sitecheck.run(args.cc, code, "oot-save-") == "ok\n", "the fixture did not run to its end")
    print("PASS: all known transition requests respect the toggle and unrelated requests pass through")


if __name__ == "__main__":
    main()
