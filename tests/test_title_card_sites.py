"""The area title card's frame is drawn in strips in the game: the hook sites that call game/hos_wide_ui.c's
hos_wide_title_prepare, _draw and _reset (#2555).

The three sites (game/sites/sites.toml): ui.titlePrepare replaces f_815505b0's call of f_8154f1c0 (a picture's record
packed into the frame's constants), ui.titleDraw is f_81550720's entry (a picture drawn; non-zero: drawn), ui.titleReset
f_8156abc0's (the constants' arena reset). Their callbacks are game/oot_wide_sites.c's. Each runs here behind its hook as
the lift lowers it (tests/sitehooks.py), with the real host helpers (tests/test_title_card_strips.py's fixture: synthetic
records, allocator and draw stand-ins) over a wide picture: one packing makes three strips, one draw draws them through
the draw function itself (its own entry hook then leaves the game's draw to it), a reset of the arena drops them, and a
16:9 picture or a site off leaves the game's one call. No game file is read.

python -I -B tests/test_title_card_sites.py [--cc clang] [--cargo cargo]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
import sitecheck  # noqa: E402
import sitehooks  # noqa: E402
import test_title_card_strips as strips  # noqa: E402
from srccut import between  # noqa: E402

PACK_AT = 0x815505b0 + 0x140   # (the bl f_8154f1c0 the prepare site replaces)
PACKER = 0x8154f1c0
DRAW = 0x81550720

FIXTURE = r'''
typedef void (*A64Fn)(Ctx *);
_Thread_local uint64_t hos_site_callee;   /* (game/sites.c's: a replaced bl's callee, which its hook stores) */
void f_81550720(Ctx *c);
static unsigned lookups;
A64Fn a64_lookup(uint64_t a) {
    lookups++;
    if (a == 0x8154f1c0ull) return f_8154f1c0;
    if (a == 0x81550720ull) return f_81550720;
    return 0;
}
@CALLBACKS@
@SLOTS@
/* f_815505b0's call of f_8154f1c0, as the lift writes it with its site */
static void pack(Ctx *c) {
@PACK@
        c->x[30] = 0x815506f4ull; f_8154f1c0(c);
@END@
}
/* f_81550720, the picture's draw: its entry site, then the game's draw */
void f_81550720(Ctx *c) {
@DRAW@
    title_original_draw(c);
}
static unsigned resets;
/* f_8156abc0, the arena's reset: its entry site, then the game's reset */
static void f_8156abc0(Ctx *c) {
@RESET@
    resets++;
}
static void hooked_prep(void){ctx.x[0]=PARAMS;ctx.x[1]=0xaaaa;ctx.x[2]=16;ctx.x[8]=OUT;ctx.x[19]=PICTURE;ctx.x[20]=PANE;pack(&ctx);memcpy(memory+PICTURE+0x48,memory+OUT,24);}
static void hooked_draw(void){ctx.x[0]=PICTURE;ctx.x[1]=PAIR;f_81550720(&ctx);}
int main(void){
 /* A wide picture: one packing makes the whole and three strips; one draw draws the three, the descriptor put back */
 setup("P_DecoFrameLT_00",0,0);title_plan_tick(640);hooked_prep();
 assert(allocations==4);assert(!memcmp(memory+OUT,descs[0],24));
 hooked_draw();assert(draws==3);for(int i=0;i<3;i++)assert(draw_ids[i]==(unsigned)i+2);
 assert(!memcmp(memory+PICTURE+0x48,descs[0],24));
 hooked_draw();assert(draws==6);
 /* Another arena's reset keeps them; their own arena's drops them, and the game draws the whole again */
 ctx.x[0]=0xbbbb;f_8156abc0(&ctx);assert(resets==1);hooked_draw();assert(draws==9);
 ctx.x[0]=0xaaaa;f_8156abc0(&ctx);assert(resets==2);assert(!(title_work>>1));
 hooked_draw();assert(draws==10&&draw_ids[9]==1);
 /* 16:9: the game's one packing and one draw */
 setup("P_DecoFrameLT_00",0,0);title_plan_tick(0);hooked_prep();assert(allocations==1);hooked_draw();assert(draws==1&&draw_ids[0]==1);
 /* The sites off: the game's own call and draw, nothing of the host's: no guest reads */
 hos_site_on_ui_titlePrepare=hos_site_on_ui_titleDraw=hos_site_on_ui_titleReset=0;
 setup("P_DecoFrameLT_00",0,0);title_plan_tick(640);lookups=guest_reads=0;hooked_prep();assert(allocations==1&&!lookups);
 hooked_draw();assert(draws==1&&draw_ids[0]==1&&!lookups);
 ctx.x[0]=0xaaaa;resets=0;f_8156abc0(&ctx);assert(resets==1&&!guest_reads);
 assert(!warnings_seen&&!guard_logs&&!trace_logs);   /* (no guard failed, no frame unknown) */
 puts("PASS: the title card's three sites: packed in strips, drawn in strips through the draw's own hook, dropped at its arena's reset; 16:9 and off as the game, the host reading nothing");
 return 0;
}
'''


def site_checks():
    prepare = sitecheck.site("ui.titlePrepare", "f_815505b0", "replace", 0x140)
    draw = sitecheck.site("ui.titleDraw", "f_81550720", "entry", can_return=True)
    reset = sitecheck.site("ui.titleReset", "f_8156abc0", "entry")
    for x in (prepare, draw, reset):
        sitecheck.engine_fn(x)
    return prepare, draw, reset


def fixture(cc, prepare, draw, reset):
    host = (ROOT / "game/hos_wide_ui.c").read_text(encoding="utf-8")
    head = strips.FIXTURE.replace("@HOST@", between(host, "/* The area title card's baked frame", "void hos_wide_ui_tick(void)"))
    head = head[:head.index("int main(void){")]
    wide = (ROOT / "game/oot_wide_sites.c").read_text(encoding="utf-8")
    callbacks = (between(wide, *sitecheck.ADDRESSES) + between(wide, "int hos_site_ui_title_prepare(", "/* ui.layoutBuilt"))
    code = head + (FIXTURE.replace("@CALLBACKS@", callbacks)
                   .replace("@SLOTS@", "".join(sitecheck.slot(x) for x in (prepare, draw, reset)))
                   .replace("@PACK@", sitehooks.line(prepare, PACK_AT, PACKER))
                   .replace("@END@", sitehooks.REPLACE_END)
                   .replace("@DRAW@", sitehooks.line(draw))
                   .replace("@RESET@", sitehooks.line(reset)))
    with tempfile.TemporaryDirectory(prefix="title-card-sites-") as tmp:
        src, exe = Path(tmp) / "fixture.c", Path(tmp) / "fixture.exe"
        src.write_text(code, encoding="utf-8")
        made = subprocess.run([cc, "-std=gnu11", "-O1", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function",
                               "-Wno-unused-variable", "-ffp-contract=off", str(src), *sitehooks.objects(tmp, code),
                               "-lm", "-o", str(exe)], capture_output=True, text=True)
        sitecheck.check(made.returncode == 0, "the fixture does not build:\n" + made.stderr)
        ran = subprocess.run([str(exe)], capture_output=True, text=True)
        sitecheck.check(ran.returncode == 0, "the fixture failed:\n" + ran.stdout + ran.stderr)
        print(ran.stdout.strip())


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    fixture(a.cc, *site_checks())


if __name__ == "__main__":
    main()
