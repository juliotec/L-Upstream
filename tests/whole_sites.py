"""The counters cut to whole frames (the hook sites frame.whole*, game/sites/sites.toml; game/oot_sites.c's callbacks over
host/nn_vi.c's hos_frame_whole) for the checks that compile authored lifted functions with them. Standard library,
tests/sitecheck.py and tests/sitehooks.py only.

    sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
    from whole_sites import hook_whole, old_whole, SITES, cut

hook_whole(text): text (a lifted file: '#include "a64.h"' and functions) with each site's cut, in its function, wrapped in the
replace site's hook as the lift lowers it (tests/sitehooks.py: the check links sitehooks.objects(tmp, its C)), and the
callbacks (cut from game/oot_sites.c), their flags (on) and slots after the include: (the text, how many cuts were hooked). A function the text does not have is left out; a cut not found exactly once in
its function (with the lines around it the site's signature stands for) is left as it is. old_whole(text): the same with the
text patch's line in place of each cut, as nx2/oot_patches.py patch_whole_frames made it on main, for a side-by-side check.
Neither declares hos_frame_whole or hos_frame_whole_times beyond a prototype: the check gives them.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sitehooks  # noqa: E402
from sitecheck import site, callback, slot, engine_fn  # noqa: E402

__all__ = ["SITES", "cut", "hook_whole", "old_whole"]

ADD_X10 = "    { uint64_t _a=c->x[10],_b=((uint64_t)(uint32_t)(((uint32_t)(c->x[8])) << 0)),_r=_a+_b; _r&=0xFFFFFFFFULL; c->x[8]=_r; }\n"
# (the site, its function, the cut's register, signed (fcvtzs) or not (fcvtzu), the lifted lines just before and after the cut
# that tell it from another cut in the function, the key the text patch carried the counter's fraction at, the register of the
# substeps for the water's)
SITES = (
    ("frame.wholePickup", "f_80a87e50", 12, 1, "", "", "c->x[19] + 864", None),
    ("frame.wholeWater", "f_800672e0", 9, 1, "    c->x[8]=recomp_load32(c,c->x[19] + 76);\n", "", "c->x[21]", "c->x[8]"),
    ("frame.wholeTimeOfDay", "f_803edf70", 8, 1, "", "    recomp_store32(c,c->x[19] + 60,c->x[8]);\n", "c->x[19] + 60", None),
    ("frame.wholeScarecrowPlay", "f_832b32b0", 8, 0, "", "", "c->x[19] + 0x265fc", None),
    ("frame.wholeScarecrowRecord", "f_832b6840", 8, 0, "", ADD_X10, "c->x[9]", None),
    ("frame.wholeScarecrowLast", "f_832b2690", 8, 0, "", "", "c->x[19] + 0x265fc", None),
    ("frame.wholeFloorWeb", "f_80652cb0", 22, 1, "", "", "c->x[19] + 2296", None),
)
DECL = ("uint32_t hos_frame_whole(uint64_t at, uint32_t amount_bits);\n"
        "uint32_t hos_frame_whole_times(uint64_t at, uint32_t amount_bits, uint32_t times);\n")
MARK = "/* (the step's whole frames: nx2/oot_patches.py) */"


def cut(reg, signed):
    """the cut of s0 to a 32-bit integer in w<reg>, as the lift writes it"""
    return "    c->x[%d]=recomp_fp_to_int(c->vreg[0][0]&UINT64_C(0xffffffff),32,32,%d,3,c->fpcr,&c->fpsr);\n" % (reg, signed)


def _function(text, function):
    head = text.find("void %s(Ctx *__restrict A64_NOESC c) {\n" % function)
    if head < 0:
        return None
    end = text.find("\nvoid f_", head + 1)
    return head, end if end >= 0 else len(text)


def _each(text, make):
    """text with each site's cut (and the lines around it) made make(site row, the cut) in its function: (text, count, rows)"""
    done = []
    for row in SITES:
        name, function, reg, signed, before, after, key, times = row
        at = _function(text, function)
        if at is None:
            continue
        body = text[at[0]:at[1]]
        old = before + cut(reg, signed) + after
        if body.count(old) != 1:
            continue
        body = body.replace(old, before + make(row) + after)
        text = text[:at[0]] + body + text[at[1]:]
        done.append(row)
    return text, done


def _top(text, extra):
    include = '#include "a64.h"\n'
    return text.replace(include, include + extra, 1) if include in text else extra + text


def hook_whole(text):
    sites = {}

    def make(row):
        x = site(row[0], row[1], "replace")
        sites[x.name] = x
        return sitehooks.line(x) + "\n" + cut(row[2], row[3]) + sitehooks.REPLACE_END + "\n"

    text, done = _each(text, make)
    if not done:
        return text, 0
    fns = []
    for x in sites.values():
        fn = "int %s(" % engine_fn(x)
        if fn not in fns:
            fns.append(fn)
    extra = DECL + "".join(callback(fn) + "\n" for fn in fns) + "".join(slot(x) for x in sites.values())
    return _top(text, extra), len(done)


def old_whole(text):
    def make(row):
        _, _, reg, _, _, _, key, times = row
        if times:
            return "    c->x[%d]=hos_frame_whole_times(%s, (uint32_t)c->vreg[0][0], (uint32_t)%s); %s=1;   %s\n" % (
                reg, key, times, times, MARK)
        return "    c->x[%d]=hos_frame_whole(%s, (uint32_t)c->vreg[0][0]);   %s\n" % (reg, key, MARK)

    text, done = _each(text, make)
    return (_top(text, DECL) if done else text), len(done)
