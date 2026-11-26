"""The game's note-wait action (ExecuteWaitOcarinaInputCommon: start f_828a9190, tick f_828a9280) reports itself to the host
(hos_oc_wait / hos_oc_last_kind / hos_oc_beat in host/nn_hid.c, for the ocarina's five note controls) through seven hook sites,
ocarina.wait* (game/sites/sites.toml, game/oot_sites.c), where the text patch they replaced (nx2/oot_patches.py
patch_ocarina_wait, deleted) put its lines. The start decides from the input kind, every running tick re-arms and stamps the
beat (the input resets between note windows keep the action running), and the not-running branch, the input-reset branch and
the done store clear it. The tick's three gates (the mode check, the input context, the wait's owner) also clear it: a tick
that refuses, say a dialogue took the buttons, must not leave the host swapping the pads.

The two functions, made up in the shape the lift writes them (the sites in their order inside the tick), are built with the
sites' hooks as the lift lowers them (tests/sitehooks.py) (after an instruction, or before one: after its label) and, side by side, with the
patch's edits of the form main made them (OLD_EDITS), and run over every input kind, flag byte, mode, context and owner: the same wait, kind and beat, and the
same registers. The input kinds that arm the note buttons are the ocarina's performances (13, a dialogue's, is not).

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_ocarina_wait.py [--cc clang] [--cargo cargo]
"""
import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import check, site, callback, slot  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

# (the text patch's edits, each (the lifted text, the text in its place), and the declarations it put at the file's head: the
# side-by-side check below runs the functions both ways. The lifted text is made up, in the lift's form: the registers and
# flags the callbacks read are the sites' (x1 loaded, bit 1 of w8 tested, z, x0 answered), and the offsets, callees, return
# addresses and labels are written here)
OLD_EDITS = (
    ('    c->x[1]=recomp_load32(c,c->x[19] + 40);\n    { uint64_t _r = ((uint64_t)0 | ((c->x[0]) << 0)); c->x[20] = _r; }\nc->x[30] = 0x80001004ULL; f_80002000(c);',
     "    c->x[1]=recomp_load32(c,c->x[19] + 40);\n    hos_oc_last_kind = (int)c->x[1], hos_oc_wait = HOS_OC_BUTTONS(c->x[1]) ? 1 : 2, hos_oc_beat++;   /* (nx2/oot_patches.py: the note wait began; 1 and 3 are the field ocarina, 2 a song lesson, 7 the Lost Woods skull kids' songs, 10 the scarecrow's recording, 11 the scarecrow's recall of it, 13 a dialogue's, which stays the game's) */\n    { uint64_t _r = ((uint64_t)0 | ((c->x[0]) << 0)); c->x[20] = _r; }\nc->x[30] = 0x80001004ULL; f_80002000(c);"),
    ('    c->x[8]=recomp_load8(c,c->x[0] + 16);\nif (!((c->x[8] >> 1) & 1)) { goto L_80001400; }',
     '    c->x[8]=recomp_load8(c,c->x[0] + 16);\nif (!((c->x[8] >> 1) & 1)) { hos_oc_wait = 0; goto L_80001400; }   /* (nx2/oot_patches.py: this action stopped running: no wait) */\n    hos_oc_wait = HOS_OC_BUTTONS(hos_oc_last_kind) ? 1 : 2, hos_oc_beat++;   /* (nx2/oot_patches.py: re-armed every running tick: the input resets between note windows end one wait without ending the action; the beat tells the host the wait is alive) */'),
    ('if (!c->z) { goto L_80001400; }',
     "if (!c->z) { hos_oc_wait = 0; goto L_80001400; }   /* (nx2/oot_patches.py: the action's mode check refused: no wait) */"),
    ('c->x[30] = 0x80001104ULL; f_80002100(c);\nif (c->x[0] != 0) { goto L_80001500; }',
     'c->x[30] = 0x80001104ULL; f_80002100(c);\nif (c->x[0] != 0) { goto L_80001500; }\nhos_oc_wait = 0;   /* (nx2/oot_patches.py: the input context is gone: no wait) */'),
    ('c->x[30] = 0x80001204ULL; f_80002200(c);\nif (c->x[0] == 0) { { A64_POLL(c); goto L_80001400; } }',
     "c->x[30] = 0x80001204ULL; f_80002200(c);\nif (c->x[0] == 0) { { A64_POLL(c); hos_oc_wait = 0; goto L_80001400; } }   /* (nx2/oot_patches.py: the wait's owner refused: no wait, and the pads are the game's again) */"),
    ('recomp_store32(c,c->x[19] + 28,c->x[9]);',
     'recomp_store32(c,c->x[19] + 28,c->x[9]);\n    hos_oc_wait = 0;   /* (nx2/oot_patches.py: the wait action finished) */'),
    ('c->x[30] = 0x80001304ULL; f_80002000(c);',
     "c->x[30] = 0x80001304ULL; f_80002000(c);\n    hos_oc_wait = 0;   /* (nx2/oot_patches.py: the input reset: this window's wait is over) */"),
)
OLD_DECL = 'extern volatile int hos_oc_wait;\nextern volatile int hos_oc_last_kind;\nextern volatile unsigned hos_oc_beat;\n#define HOS_OC_BUTTONS(k) ((k) == 1 || (k) == 2 || (k) == 3 || (k) == 7 || (k) == 10 || (k) == 11)\n'

# (the shape of the two lifted functions: entry with the mode load, tick with its running gate, its three gates, the done
# store and the input-reset branch, in the order of the sites' offsets inside the tick)
ENTRY_LOAD = "    c->x[1]=recomp_load32(c,c->x[19] + 40);\n"
LIFTED = """void f_828a9190(Ctx *__restrict A64_NOESC c) {
""" + OLD_EDITS[0][0] + """
return;
}
void f_828a9280(Ctx *__restrict A64_NOESC c) {
""" + OLD_EDITS[1][0] + """
""" + OLD_EDITS[2][0] + """
""" + OLD_EDITS[3][0] + """
L_80001500:; A64_POLL(c);
""" + OLD_EDITS[4][0] + """
L_80001600:; A64_POLL(c);
""" + OLD_EDITS[5][0] + """
""" + OLD_EDITS[6][0] + """
L_80001400:; A64_POLL(c);
return;
}
"""
# (where each site's hook goes in LIFTED: after the text or before it. The reset's is a before site on the instruction after
# the call, which nothing else reaches: right after the call's text)
PLACES = (("ocarina.waitStart", "f_828a9190", "after", 0x48, ENTRY_LOAD, "after"),
          ("ocarina.waitTick", "f_828a9280", "before", 0x1c, "if (!((c->x[8] >> 1) & 1)) { goto L_80001400; }", "before"),
          ("ocarina.waitMode", "f_828a9280", "before", 0x4c, "if (!c->z) { goto L_80001400; }", "before"),
          ("ocarina.waitContext", "f_828a9280", "before", 0x60, "if (c->x[0] != 0) { goto L_80001500; }", "before"),
          ("ocarina.waitOwner", "f_828a9280", "before", 0x98, "if (c->x[0] == 0) { { A64_POLL(c); goto L_80001400; } }", "before"),
          ("ocarina.waitDone", "f_828a9280", "after", 0x108, "recomp_store32(c,c->x[19] + 28,c->x[9]);", "after"),
          ("ocarina.waitReset", "f_828a9280", "before", 0x164, "c->x[30] = 0x80001304ULL; f_80002000(c);", "after"))

SIDE = r"""#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct { uint64_t x[32]; uint8_t z; } Ctx;
#define A64_NOESC
#define A64_POLL(c) ((void)0)
static uint64_t in_kind, in_flags, in_context, in_owner;
static volatile uint64_t stored;   /* (written, never read: volatile, so clang does not warn) */
static uint64_t recomp_load32(Ctx *c, uint64_t a) { (void)c; (void)a; return in_kind; }
static uint64_t recomp_load8(Ctx *c, uint64_t a) { (void)c; (void)a; return in_flags; }
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { (void)c; (void)a; stored = v; }
static void f_80002000(Ctx *c) { c->x[2] = 0x35d0; }
static void f_80002100(Ctx *c) { c->x[0] = in_context; }
static void f_80002200(Ctx *c) { c->x[0] = in_owner; }
volatile int hos_oc_wait, hos_oc_last_kind;
volatile unsigned hos_oc_beat;
@NEW@
@OLD@
int main(void) {
    int fails = 0;
    for (in_kind = 0; in_kind < 15; in_kind++)
        for (in_flags = 0; in_flags < 4; in_flags++)
            for (int z = 0; z < 2; z++)
                for (in_context = 0; in_context < 2; in_context++)
                    for (in_owner = 0; in_owner < 2; in_owner++)
                        for (int w = 0; w < 3; w++) {
                            Ctx a, b;
                            memset(&a, 0, sizeof a);
                            for (int r = 0; r < 31; r++) a.x[r] = 0x9e3779b97f4a7c15ull * (r + 1);
                            a.z = (uint8_t)z;
                            b = a;
                            int wa[3], wb[3];
                            hos_oc_wait = w, hos_oc_last_kind = 5, hos_oc_beat = 7;
                            f_828a9190(&a), f_828a9280(&a);
                            wa[0] = hos_oc_wait, wa[1] = hos_oc_last_kind, wa[2] = (int)hos_oc_beat;
                            hos_oc_wait = w, hos_oc_last_kind = 5, hos_oc_beat = 7;
                            old_828a9190(&b), old_828a9280(&b);
                            wb[0] = hos_oc_wait, wb[1] = hos_oc_last_kind, wb[2] = (int)hos_oc_beat;
                            if (memcmp(wa, wb, sizeof wa) || memcmp(&a, &b, sizeof a)) {
                                printf("FAIL kind %d flags %d z %d context %d owner %d wait %d\n", (int)in_kind, (int)in_flags,
                                       z, (int)in_context, (int)in_owner, w);
                                fails++;
                            }
                        }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""


def hooked():
    """LIFTED with each site's hook where the emit puts it, the callbacks and their slots before"""
    text, callbacks, slots = LIFTED, [], ""
    for name, function, at, offset, anchor, put in PLACES:
        x = site(name, function, at, offset)
        check(text.count(anchor) == 1, "%s's place is not once in the authored functions" % name)
        hook = sitehooks.line(x) + "\n"
        text = text.replace(anchor, anchor + "\n" + hook if put == "after" else hook + anchor, 1)
        fn = slot(x).split(") = ")[1].split(";")[0]
        if fn not in callbacks:
            callbacks.append(fn)
        slots += slot(x)
    return ("#define HOS_OC_BUTTONS(k) ((k) == 1 || (k) == 2 || (k) == 3 || (k) == 7 || (k) == 10 || (k) == 11)\n"
            + "".join(callback("int %s(" % fn) + "\n" for fn in callbacks) + slots + text)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    calls = (Path(__file__).resolve().parents[1] / "game" / "oot_sites.c").read_text(encoding="utf-8")
    kinds = {int(k) for k in re.findall(r"\(k\) == (\d+)", calls.split("#define HOS_OC_BUTTONS(k)")[1].split("\n")[0])}
    check(kinds == {1, 2, 3, 7, 10, 11}, "the kinds that arm the note buttons are not the ocarina's performances: "
          + " ".join(str(k) for k in sorted(kinds)))
    old = LIFTED
    for before, after in OLD_EDITS:
        check(old.count(before) == 1, "an edit of the text patch is not once in the authored functions")
        old = old.replace(before, after, 1)
    old = old.replace("void f_828a9", "void old_828a9")
    cc = shutil.which(a.cc)
    if not cc:
        print("no C compiler: the side-by-side run skipped")
        return
    code = SIDE.replace("@NEW@", hooked()).replace("@OLD@", OLD_DECL.replace("extern volatile int hos_oc_wait;\n", "").replace(
        "extern volatile int hos_oc_last_kind;\n", "").replace("extern volatile unsigned hos_oc_beat;\n", "").replace(
        "#define HOS_OC_BUTTONS", "#undef HOS_OC_BUTTONS\n#define HOS_OC_BUTTONS") + old)
    with tempfile.TemporaryDirectory(prefix="oc-wait-") as tmp:
        f, exe = Path(tmp) / "t.c", Path(tmp) / "t.exe"
        f.write_text(code, encoding="utf-8")
        subprocess.run([cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-label", str(f),
                        *sitehooks.objects(tmp, code), "-o", str(exe)], check=True)
        r = subprocess.run([str(exe)], text=True, capture_output=True)
        check(r.returncode == 0, "the sites and the text patch differ:\n" + r.stdout + r.stderr)
    print("PASS: the note wait reports itself through its sites as through the text patch: the same wait, kind, beat and registers")


if __name__ == "__main__":
    main()
