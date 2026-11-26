"""The hook site tables (game/sites/sites.toml, game/sites/<build id>/locations.toml, game/sites.def) as the
site tests read them, and the checks a feature's hook sites (docs/dev/reference/hooks.md) make of them, game-free.
Standard library only. The builder's own reader and its every rule of the format are builder/crates/sites; here a site
is only what a check needs of it: its name, form and place. Its hook as the lifted code runs it is tests/sitehooks.py's
(the real lowering, through longshot-builder dev site-hooks).

    sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
    from sitecheck import site, callback

load(build): the build's sites (definitions joined with its locations, in the definitions' order), None for a build
with no table; host_sites(), engine_sites(), engine_callback(ident), host_values(): game/sites.def's lines.
site(name, function, at, offset=None, can_return=False): the supported build's site of that name, at that place and of
that form, with its host slot (game/sites.def); the Site. callback(marker, source="game/oot_sites.c"): the C of a
callback of that host file, from its marker to its closing brace, after the file's block of the build's address names.
slot(x, on=1): C defining the site's flag (on or off) and its slot, holding the engine's callback game/sites.def
names for it (a fixture defines or cuts the callback before it); engine_fn(x): that callback's name. hooked(x, marker):
the callback cut at marker with its flag and slot, and the site's hook (sitehooks.line). run(cc, code): a fixture's C with
hooks in it, compiled with cc and the hooks' object, run: its output. looks(cc, sites, stubs): each (site, marker) of
a site whose hook only hands registers to the host, its callback cut at marker, run in a function of its own over
known registers (x[r] = REG + r), with the site on and off, against stubs that record what they are handed: what each
was handed (Looked). hands(cc, sites, stubs): looks() over (site, marker, handed) and the check that each hook, on,
calls once, hands exactly handed (its values; CTX for the context; reg(r) for register r) and goes on, and off calls
nothing and goes on.

A failed check stops with SystemExit("FAIL: ...").
"""
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))
import sitehooks  # noqa: E402
from srccut import block, between  # noqa: E402

BUILD = sitehooks.BUILD   # (the supported dump's build: the one table)
GAME = ROOT / "game" / "sites"   # (each build's folder: its locations.toml)
DEFS = ROOT / "game" / "sites" / "sites.toml"   # (each site's definition: the same for every build)
SITES_DEF = ROOT / "game" / "sites.def"   # (the host's flag and slot for each site)
TABLE = "locations.toml"

__all__ = ["BUILD", "ROOT", "check", "site", "callback", "slot", "engine_fn", "hooked", "load", "host_sites",
           "engine_sites", "engine_callback", "host_values", "ident", "Site", "run", "looks", "Looked", "REG", "hands", "CTX", "reg"]


WORD = re.compile(r"^(?:(?P<nibbles>[0-9a-f?]{8})|(?P<value>[0-9a-f]{8})/(?P<mask>[0-9a-f]{8}))$")


def parse_signature(text):
    """A signature's text (instruction words as 8 hex digits, ? a masked nibble, value/mask masked bits): ((value, mask),
    ...)"""
    out = []
    for w in text.lower().split():
        m = WORD.match(w)
        if not m:
            raise ValueError("%r is not a signature's word" % w)
        if m.group("nibbles"):
            n = m.group("nibbles")
            out.append((int(n.replace("?", "0"), 16), int("".join("0" if ch == "?" else "f" for ch in n), 16)))
        else:
            out.append((int(m.group("value"), 16), int(m.group("mask"), 16)))
    return tuple(out)


class Anchor:
    """A site's point found by what is there (docs/dev/reference/hooks.md#anchors): kind (words, call, vcall or string),
    words and at (words), target (call, vcall, string), start and end (from and to: a site's name, or an Anchor), ordinal,
    of (with ordinal: how many matches the slice must hold), shift"""
    KINDS = ("words", "call", "vcall", "string")

    def __init__(self, raw):
        self.kind = next(k for k in self.KINDS if k in raw)
        self.words = parse_signature(raw["words"]) if self.kind == "words" else None
        self.at = raw.get("at", 0)
        self.target = None if self.kind == "words" else raw[self.kind]
        self.start, self.end = (raw.get(k) if not isinstance(raw.get(k), dict) else Anchor(raw[k]) for k in ("from", "to"))
        self.ordinal, self.of, self.shift = raw.get("ordinal"), raw.get("of"), raw.get("shift", 0)

    def describe(self):
        return "%s %r%s%s" % (self.kind, self.words if self.kind == "words" else self.target,
                              "" if self.ordinal is None else " (number %d%s)" % (
                                  self.ordinal, "" if self.of is None else " of %d" % self.of),
                              ", %+d instructions" % self.shift if self.shift else "")


def ident(name):
    """A site's C identifier: its name with each dot an underscore (hos_site_on_<ident>, hos_site_fn_<ident>)"""
    return name.replace(".", "_")


class Site:
    """One site of a build: its definition with its location"""
    __slots__ = ("name", "kind", "doc", "function", "at", "offset", "signature", "signature_offset", "shape", "thread",
                 "args", "returns", "module", "vtable", "slot", "can_return", "rule", "value", "to", "guards", "inside",
                 "point", "fingerprint", "require")

    def __init__(self, raw):
        self.name, self.kind, self.doc = raw["name"], raw.get("kind", "code"), raw.get("doc")
        self.function, self.at = raw.get("function"), raw.get("at", "entry")
        self.offset = raw.get("offset", 0 if "point" not in raw else None)
        sig = raw.get("signature")
        self.signature = parse_signature(sig) if sig is not None else None
        self.guards = tuple((g["offset"], parse_signature(g["signature"])) for g in raw.get("guards", ()))
        self.signature_offset = raw.get("signature_offset", 0)
        self.shape, self.thread = raw.get("shape"), raw.get("thread")
        self.args, self.returns, self.module = tuple(raw.get("args", ())), raw.get("returns", "void"), raw.get("module", "main")
        self.vtable, self.slot = raw.get("vtable"), raw.get("slot")
        self.can_return, self.rule, self.value = bool(raw.get("can_return", False)), raw.get("rule"), raw.get("value")
        self.to, self.inside, self.fingerprint = raw.get("to"), raw.get("in"), raw.get("fingerprint")
        self.point = Anchor(raw["point"]) if "point" in raw else None
        self.require = raw.get("require", True)

    @property
    def ident(self):
        return ident(self.name)

    def __repr__(self):
        return "Site(%s)" % self.name


def _sites(path):
    with open(path, "rb") as f:
        return tomllib.load(f).get("site", [])


def definitions(path=DEFS):
    """[raw definition, ...] in the file's order"""
    return _sites(path)


def table_path(build_id, game=GAME):
    return Path(game) / build_id / TABLE


def load(build_id, game=GAME, defs=DEFS):
    """The build's sites, in the definitions' order (the order of several hooks at one instruction); None when the
    repository has no locations for it. A required site the build does not locate, or a location of no site, raises
    ValueError"""
    build_id = (build_id or "").lower()[:40]
    path = table_path(build_id, game)
    if not build_id or not path.is_file():
        return None
    locs = {r["name"]: r for r in _sites(path)}
    names = [d["name"] for d in definitions(defs)]
    unknown = sorted(set(locs) - set(names))
    if unknown:
        raise ValueError("%s: %s is located but not defined" % (path, unknown[0]))
    out = []
    for d in definitions(defs):
        loc = locs.get(d["name"])
        if loc is None:
            if d.get("require", True):
                raise ValueError("%s: %s is required and not located" % (path, d["name"]))
            continue
        out.append(Site(dict(d, **loc)))
    return out


def site_lines(path=SITES_DEF):
    """[(ident, name, engine callback or None)] of game/sites.def's HOS_SITE and HOS_SITE_ENGINE lines, in order:
    the one reader of them"""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    return [(i, n, fn or None) for engine, i, n, fn in
            re.findall(r'^HOS_SITE(_ENGINE)?\((\w+),\s*"([^"]+)"(?:,\s*(\w+))?\)', text, re.M)
            if bool(engine) == bool(fn)]


def host_sites(path=SITES_DEF):
    """{ident: name} of the sites the host gives a flag and a slot (game/sites.def's HOS_SITE lines)"""
    return {i: n for i, n, _ in site_lines(path)}


def engine_sites(path=SITES_DEF):
    """The names of the sites the engine's own features own (game/sites.def's HOS_SITE_ENGINE lines)"""
    return {n for _, n, fn in site_lines(path) if fn}


def engine_callback(ident, path=SITES_DEF):
    """The engine's callback sites.def names for the site of this identifier, or None"""
    return next((fn for i, _, fn in site_lines(path) if i == ident and fn), None)


def host_values(path=SITES_DEF):
    """{name: (ident, C type, symbol)} of the values the host gives the rules (game/sites.def's HOS_VALUE lines)"""
    with open(path, encoding="utf-8") as f:
        return {n: (i, t, sym) for i, n, t, sym in
                re.findall(r'^HOS_VALUE\((\w+),\s*"([^"]+)",\s*(\w+),\s*(\w+)\)', f.read(), re.M)}


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def site(name, function, at="entry", offset=None, can_return=False):
    x = {s.name: s for s in load(BUILD)}.get(name)
    check(x is not None, "the table has no site %s" % name)
    check((x.function, x.at) == (function, at), "%s is not at %s's %s (%s, %s)" % (name, function, at, x.function, x.at))
    check(offset is None or x.offset == offset, "%s is not at %s+%#x" % (name, function, offset or 0))
    check(bool(x.can_return) == bool(can_return), "%s %s return from the function" % (name, "cannot" if can_return else "can"))
    if not x.rule:
        check(host_sites().get(x.ident) == name, "game/sites.def has no HOS_SITE line for %s" % name)
    return x


ADDRESSES = ("/* ---- build e0f3ccfa's addresses", "/* ---- end of the build's addresses")


def callback(marker, source="game/oot_sites.c"):
    """the callback's C, after the build's address names it may use (each cut repeats them: a macro defined the same twice is
    one). source: the host file it is in (game/oot_wide_sites.c for widescreen's)"""
    text = (ROOT / source).read_text(encoding="utf-8")
    return between(text, *ADDRESSES) + block(text, marker)


def engine_fn(x):
    fn = engine_callback(x.ident)
    check(fn is not None, "game/sites.def has no HOS_SITE_ENGINE line for %s" % x.name)
    return fn


def slot(x, on=1):
    return ("volatile uint8_t hos_site_on_%s = %d;\nint (*volatile hos_site_fn_%s)(Ctx *) = %s;\n"
            % (x.ident, on, x.ident, engine_fn(x)))


def hooked(x, marker):
    """(the C a fixture puts at file scope before the hooked code: the site's callback, cut from game/oot_sites.c at marker,
    and its flag (on) and slot; the hook's line at the site, as the lifted code runs it: sitehooks.line)"""
    return ("#ifndef A64_NOESC\n#define A64_NOESC\n#endif\n" + callback(marker) + "\n" + slot(x), sitehooks.line(x))


def run(cc, code, prefix="site-fixture-"):
    """code (a fixture's C, its hooks sitehooks.line's) compiled with cc and the hooks' object (sitehooks.objects), and
    run: its standard output. A fixture that does not build, or that exits non-zero, stops the check"""
    with tempfile.TemporaryDirectory(prefix=prefix) as tmp:
        src, exe = Path(tmp) / "fixture.c", Path(tmp) / "fixture.exe"
        src.write_text(code, encoding="utf-8")
        made = subprocess.run([cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-label", str(src),
                               *sitehooks.objects(tmp, code), "-o", str(exe)], capture_output=True, text=True)
        check(made.returncode == 0, "the fixture does not build:\n" + made.stderr)
        ran = subprocess.run([str(exe)], capture_output=True, text=True)
        check(ran.returncode == 0, "the fixture failed:\n" + ran.stdout + ran.stderr)
        return ran.stdout


REG = 0x1000   # (looks(): each register x[r] holds REG + r when the hook runs)


class Looked:
    """What a site's run in looks() saw: calls (how many times the stubs were called), went_on (the function went on past
    the hook), ctx (the context's address), got (the stubs' record: got[0..7] as they set it)"""
    __slots__ = ("calls", "went_on", "ctx", "got")

    def __init__(self, calls, went_on, ctx, *got):
        self.calls, self.went_on, self.ctx, self.got = calls, went_on, ctx, got

    def __repr__(self):
        return "Looked(calls=%d, went_on=%d, ctx=%#x, got=%s)" % (self.calls, self.went_on, self.ctx,
                                                                  [hex(g) for g in self.got])


LOOKS = r"""#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; } Ctx;
static int calls;   /* (the stubs: each call adds one, and puts what it was handed in got) */
static uint64_t got[8];
@STUBS@
@CALLBACKS@
static int went_on;
@SITES@
static void run(void (*f)(Ctx *), volatile uint8_t *on, int flag) {
    Ctx c;
    for (int r = 0; r < 32; r++) c.x[r] = @REG@u + r;
    calls = went_on = 0, memset(got, 0, sizeof got), *on = (uint8_t)flag;
    f(&c);
    printf("%d %d %llu", calls, went_on, (unsigned long long)(uintptr_t)&c);
    for (int i = 0; i < 8; i++) printf(" %llu", (unsigned long long)got[i]);
    printf("\n");
}
int main(void) {
@RUNS@
    return 0;
}
"""


def looks(cc, sites, stubs):
    """[(on, off)] for each (x, marker) of sites, each a Looked: the site's hook (sitehooks.line), as the lift lowers it,
    first in a function that then goes on, its callback cut from game/oot_sites.c at marker (each marker once) and the
    stubs (C defining what the callbacks call; each adds one to calls and records its arguments in got, a uint64_t[8],
    the context as (uint64_t)(uintptr_t)c), run over registers x[r] = REG + r with the site's flag on, then off"""
    markers, defs, fns, runs = [], "", "", ""
    for i, (x, marker) in enumerate(sites):
        if marker not in markers:
            markers.append(marker)
        defs += slot(x)
        fns += "static void site_%d(Ctx *c) {\n%s\n    went_on = 1;\n}\n" % (i, sitehooks.line(x))
        runs += "    run(site_%d, &hos_site_on_%s, 1), run(site_%d, &hos_site_on_%s, 0);\n" % (i, x.ident, i, x.ident)
    code = (LOOKS.replace("@STUBS@", stubs).replace("@CALLBACKS@", "".join(callback(m) + "\n" for m in markers) + defs)
            .replace("@SITES@", fns).replace("@RUNS@", runs).replace("@REG@", "%#x" % REG))
    rows = [Looked(*map(int, line.split())) for line in run(cc, code).splitlines()]
    check(len(rows) == 2 * len(sites), "the fixture ran %d of %d sites" % (len(rows) // 2, len(sites)))
    return [(rows[2 * i], rows[2 * i + 1]) for i in range(len(sites))]


CTX = "ctx"   # (hands(): the context, where a stub records it)


def reg(r):
    """hands(): the value register x[r] held when the hook ran"""
    return REG + r


def hands(cc, sites, stubs):
    """Each (x, marker, handed) of sites, through looks(): on, its callback calls the stubs once with handed (CTX the
    context) and the function goes on; off, nothing is called and the function goes on"""
    seen = looks(cc, [(s, m) for s, m, _ in sites], stubs)
    for (s, _, handed), (on, off) in zip(sites, seen):
        want = tuple(on.ctx if h == CTX else h for h in handed)
        check(on.calls == 1 and on.got[:len(want)] == want and not any(on.got[len(want):]),
              "%s's hook does not hand the host %s: %r" % (s.name, [hex(w) for w in want], on))
        check(on.went_on, "%s's hook leaves the function" % s.name)
        check(off.calls == 0 and off.went_on, "%s's hook, off, does not leave the function as the game has it: %r" % (s.name, off))
