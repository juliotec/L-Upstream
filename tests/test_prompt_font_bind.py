"""Game-free: the button prompts' font patch at run time (host/prompts.c cmap4_glyph, cmap4_bind) on a cmap made the way the
build makes it (builder/crates/build-assets/src/font/mod.rs cmap4, every face's icons; the prompts' constants read from
builder/crates/build-assets/src/prompts/): a bound Switch button's code point gets the face's icon of its
pad input, an unbound one the face's icon of its default's, the d-pad icons the face's, and the faces' icons stay. And a
text's codes after a change while the game runs (host/prompts.c text_code): a Switch button's, by the game or rewritten
before, the code of the face's icon of its pad input now (build-assets' BIND_BASE), the d-pad's the face's. The
keyboard face (build-assets' prompts/keys.rs): a Switch button gets its first key's icon (the second's when the first slot is clear), one on
a key without an icon or on none the face's icon of its pad input; the sticks the keyboard's icons (the other faces the game's
own); in a text, a button on a key a code of its own (key_slot), the sticks' codes the face's. The left stick's icons follow the
movement keys (MOVES): one way shows its key's icon, in a text through a code of its own (unbound or a key without an icon:
the game's stick icon), several ways the W A S D picture only while those are the keys, and current_binds notices a movement
key changed without the buttons' part changing. Also the faces' order: host/prompts.c faces[], gpu/vk/vk_api.h HOS_PROMPT_*
and build-assets' FACES (after the Switch, then the keyboard), and the defaults: host/prompts.c switch_pad, static_code,
stick_code, stick_ways and move_default, build-assets' DEFAULT and STATIC, STICKS, move_key and the code points.

python -I -B tests/test_prompt_font_bind.py [--cc gcc]
"""
import argparse
import os
import re
import struct
import subprocess
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "gcc"]

ROOT = Path(__file__).resolve().parents[1]
PROMPTS = ROOT / "builder" / "crates" / "build-assets" / "src" / "prompts"


class Prompts:
    """The button prompts' constants as the builder has them (builder/crates/build-assets/src/prompts/), read from its source"""
    def __init__(self):
        faces, keys, glyphs = ((PROMPTS / f).read_text(encoding="utf-8") for f in ("faces.rs", "keys.rs", "glyphs.rs"))

        def const(src, name):
            m = re.search(r"pub const %s: [^=]+=\s*(.*?);\n" % name, src, re.S)
            if not m:
                raise SystemExit("FAIL: build-assets' prompts have no %s: update this check" % name)
            return m.group(1)
        num = lambda src, name: int(const(src, name), 0)
        words = lambda src, name: re.findall(r'"([^"]*)"', const(src, name))
        self.PADS, self.DEFAULT = words(faces, "PADS"), words(faces, "DEFAULT")
        self.SWITCH = [int(c, 16) for c in re.findall(r"0x([0-9A-Fa-f]+)", const(faces, "SWITCH"))]
        self.STATIC = [int(c, 16) for c in re.findall(r"\(0x([0-9A-Fa-f]+),", const(faces, "STATIC"))]
        self.FACES = re.findall(r'^        name: "([a-z0-9]+)",', faces[faces.index("pub const FACES"):], re.M)
        self.ALL_FACES = ["switch"] + self.FACES + ["keyboard"]
        self.PAD_BASE, self.FACE_CODES = num(glyphs, "PAD_BASE"), num(glyphs, "FACE_CODES")
        self.STATIC_AT, self.BIND_BASE = num(glyphs, "STATIC_AT"), num(glyphs, "BIND_BASE")
        self.KEY_BASE, self.SLOT_BASE, self.SLOTS = num(keys, "KEY_BASE"), num(keys, "SLOT_BASE"), num(keys, "SLOTS")
        self.STICK_KEYS, self.STICK_GAME = num(keys, "STICK_KEYS"), num(keys, "STICK_GAME")
        self.STICKS = [(int(c, 16), s.lower(), w) for c, s, w in
                       re.findall(r'\(0x([0-9A-Fa-f]+), Stick::(\w+), "([udlr]+)"\)', const(keys, "STICKS"))]
        m = re.search(r"pub fn move_key\(way: char\) -> u32 \{\s*match way \{\s*'u' => (0x\w+),\s*'d' => (0x\w+),\s*"
                      r"'l' => (0x\w+),\s*_ => (0x\w+),", keys)
        if not m:
            raise SystemExit("FAIL: build-assets' move_key is not u d l, then r: update this check")
        self.MOVE = dict(zip("udlr", (int(v, 16) for v in m.groups())))

    @staticmethod
    def cmap4(mapping, single):
        """a cmap of one format 4 subtable from {code: glyph id}, as build-assets' font::cmap4 writes it: runs of codes with
        consecutive glyphs share a segment, each code of `single` has one of its own; every segment by idDelta alone"""
        segs = []
        for c in sorted(mapping):
            g = mapping[c]
            if segs and c not in single and segs[-1][1] == c - 1 and segs[-1][1] not in single and \
                    (segs[-1][2] + c) & 0xFFFF == g:
                segs[-1][1] = c
            else:
                segs.append([c, c, (g - c) & 0xFFFF])
        segs.append([0xFFFF, 0xFFFF, 1])
        n = len(segs)
        search = 2 * (1 << (n.bit_length() - 1))
        sub = struct.pack(">HHHHHHH", 4, 16 + 8 * n, 0, 2 * n, search, (search // 2).bit_length() - 1, 2 * n - search)
        sub += b"".join(struct.pack(">H", s[1]) for s in segs) + b"\0\0"
        sub += b"".join(struct.pack(">H", s[0]) for s in segs)
        sub += b"".join(struct.pack(">H", s[2]) for s in segs)
        sub += b"\0\0" * n
        return struct.pack(">HHHHIHHI", 0, 2, 0, 3, 20, 3, 1, 20) + sub


P = Prompts()

# (the Switch buttons bound in the test: A to the bottom face button, B to the right shoulder; the rest unbound)
BIND = {0: 0, 1: 10, 3: 17}   # (Y on a paddle, HOS_PAD_RPADDLE1: no artwork, so its default's icon)
ART = lambda b: b in BIND and BIND[b] < len(P.PADS)   # (a binding the prompt font has an icon for)
# (and their keys: A on E, B on the mouse's left button in the second slot (the first clear), X on a key the font has no icon for;
# the rest none)
KEYS = {0: (0x45, 0), 1: (0, 0x01), 2: (0x07, 0)}
KEY_ICONS = (0x45, 0x01, 0x57, 0x53, 0x41, 0x44, 0x49)   # (the keys with an icon in the test's cmap)
KEYBOARD = len(P.FACES) + 1   # (HOS_PROMPT_KEYBOARD)
WAYS = "udlr"   # (HOS_KEY_MOVE_UP, _DOWN, _LEFT, _RIGHT)
WASD = tuple((P.MOVE[w], 0) for w in WAYS)
# (the movement keys' two slots, up down left right: the defaults; up on I; up on I in the second slot alone; up on a key the
# font has no icon for; up on no key)
MOVES = {"W A S D": WASD, "up on I": ((0x49, 0),) + WASD[1:], "up on I, second slot": ((0, 0x49),) + WASD[1:],
         "up on a key without an icon": ((0x07, 0),) + WASD[1:], "up unbound": ((0, 0),) + WASD[1:]}


def first_key(b):
    k = KEYS.get(b, (0, 0))
    return k[0] or k[1]


def stick_shows(i, move):
    """what stick icon i shows on the keyboard face with these movement keys: ("keys", i) the keyboard's picture as built,
    ("key", its code) a key's icon, or ("game", i) the game's own stick icon"""
    _, stick, ways = P.STICKS[i]
    first = {w: k[0] or k[1] for w, k in zip(WAYS, move)}
    if stick == "cam" or all(first[w] == P.MOVE[w] for w in ways):
        return ("keys", i)
    return ("key", first[ways]) if len(ways) == 1 and first[ways] in KEY_ICONS else ("game", i)


def move_env(move):
    return dict(os.environ, MOVE_KEYS=" ".join("%d %d" % k for k in move))


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="gcc")
    a = p.parse_args()
    src = (ROOT / "host" / "prompts.c").read_text(encoding="utf-8")
    api = (ROOT / "gpu" / "vk" / "vk_api.h").read_text(encoding="utf-8")

    faces = re.search(r"faces\[HOS_PROMPT_N\] = \{([^}]*)\}", src)
    check(faces is not None, "host/prompts.c has no faces[HOS_PROMPT_N]")
    names = re.findall(r'"([a-z0-9]+)"', faces.group(1))
    check(names == P.ALL_FACES and names[-1] == "keyboard",
          "faces[] %s is not the Switch, build-assets' FACES, then the keyboard" % names)
    enum = re.search(r"enum \{ (HOS_PROMPT_SWITCH[^}]*)\}", api)
    check(enum is not None, "gpu/vk/vk_api.h has no HOS_PROMPT_* enum")
    ids = [e.strip() for e in enum.group(1).split(",") if e.strip()]
    check(ids == ["HOS_PROMPT_" + n.upper() for n in names] + ["HOS_PROMPT_N"], "HOS_PROMPT_* %s is not faces[]" % ids)

    pads = re.search(r"switch_pad\[HOS_SW_N\] = \{([^}]*)\}", src)
    check(pads is not None, "host/prompts.c has no switch_pad[HOS_SW_N]")
    check([e.strip() for e in pads.group(1).split(",")] == ["HOS_PAD_" + p.upper() for p in P.DEFAULT],
          "host/prompts.c switch_pad is not build-assets DEFAULT")
    statics = re.search(r"static_code\[3\] = \{([^}]*)\}", src)
    check(statics is not None and [int(c, 16) for c in statics.group(1).split(",")] == list(P.STATIC),
          "host/prompts.c static_code is not build-assets STATIC")
    check(len(P.PADS) <= P.STATIC_AT and P.STATIC_AT + len(P.STATIC) <=
          P.FACE_CODES, "a face's pad icons and d-pad icons overlap or pass its code points")
    sticks = re.search(r"stick_code\[15\] = \{([^}]*)\}", src)
    check(sticks is not None and [int(c, 16) for c in sticks.group(1).split(",")] == [c for c, _, _ in P.STICKS],
          "host/prompts.c stick_code is not build-assets' keys.rs STICKS")
    for name, value in (("SLOT_BASE", P.SLOT_BASE), ("SLOTS", P.SLOTS), ("STICK_KEYS", P.STICK_KEYS),
                        ("STICK_GAME", P.STICK_GAME)):
        d = re.search(r"#define %s (0x[0-9A-Fa-f]+|\d+)" % name, src)
        check(d is not None and int(d.group(1), 0) == value, "host/prompts.c %s is not build-assets' keys.rs's" % name)
    check(P.KEY_BASE == P.PAD_BASE + P.FACE_CODES * len(names),
          "build-assets' keys.rs KEY_BASE is not past every face's code points (host/prompts.c KEY_BASE)")
    check(P.KEY_BASE + 256 <= P.STICK_KEYS and P.STICK_KEYS + 16 <= P.STICK_GAME and
          P.SLOT_BASE + P.SLOTS <= P.BIND_BASE, "the keyboard face's code points overlap")

    # the codes: the Switch buttons on glyphs 100.., the d-pad's on 150.., each face's pad icons on 200 + 100 x face.., its d-pad
    # icons on 290 + 100 x face.. (as faces_font: each Switch and d-pad code a segment of its own)
    nfaces = len(names)
    mapping = {c: 100 + b for b, c in enumerate(P.SWITCH)}
    mapping.update({c: 150 + k for k, c in enumerate(P.STATIC)})
    for f in range(nfaces):
        base = P.PAD_BASE + P.FACE_CODES * f
        mapping.update({base + i: 200 + 100 * f + i for i in range(len(P.PADS))})
        mapping.update({base + P.STATIC_AT + k: 290 + 100 * f + k for k in range(len(P.STATIC))})
    # (the keyboard's: its keys' icons on 900 + key, the sticks on 1200.. (theirs; the left stick's one way on its default key's
    # icon, as the build places it) and 1300.. (the game's), each stick a segment of its own on 1400..)
    stick_codes = [c for c, _, _ in P.STICKS]
    mapping.update({P.KEY_BASE + k: 900 + k for k in KEY_ICONS})
    mapping.update({c: 1400 + i for i, c in enumerate(stick_codes)})
    mapping.update({P.STICK_KEYS + i: 1200 + i for i in range(len(stick_codes))})
    mapping.update({P.STICK_KEYS + i: 900 + P.MOVE[w] for i, (_, stick, w) in enumerate(P.STICKS)
                    if stick == "move" and len(w) == 1})
    mapping.update({P.STICK_GAME + i: 1300 + i for i in range(len(stick_codes))})
    cmap = P.cmap4(mapping, set(P.SWITCH) | set(P.STATIC) | set(stick_codes))
    sub = cmap[20:]   # (one format 4 subtable, after the cmap's header and its two records)
    codes = list(P.SWITCH) + list(P.STATIC) + [P.PAD_BASE + i for i in
                                                                       range(P.FACE_CODES * nfaces)] + stick_codes

    harness = "\n".join([
        "#include <stdio.h>", "#include <stdlib.h>", "#include <string.h>", '#include "gpu/vk/vk_api.h"',
        "#define PAD_BASE 0xF700", "#define FACE_CODES %d" % P.FACE_CODES,
        "#define STATIC_AT %d" % P.STATIC_AT, *defines(src),
        "_Static_assert(HOS_PAD_ART_N == %d, \"vk_api.h HOS_PAD_ART_N is not build-assets PADS\");" % len(P.PADS),
        "static int move_key[4][2];",
        "static int hos_binding(int set, int button, int slot) {",
        "    if (set == HOS_BIND_KEY) {",
        "        if (button >= HOS_KEY_MOVE_UP && button <= HOS_KEY_MOVE_RIGHT) return move_key[button - HOS_KEY_MOVE_UP][slot];",
        *("        if (button == %d) return slot ? %d : %d;" % (b, k[1], k[0]) for b, k in KEYS.items()),
        "        return 0;",
        "    }",
        *("    if (button == %d) return %d;" % kv for kv in BIND.items()),
        "    return -1;",
        "}",
        re.search(r"static const uint16_t switch_code\[HOS_SW_N\] = \{[^}]*\};", src).group(0),
        re.search(r"static const unsigned char switch_pad\[HOS_SW_N\] = \{[^}]*\};", src).group(0),
        re.search(r"static const uint16_t static_code\[3\] = \{[^}]*\};", src).group(0),
        re.search(r"static const uint16_t stick_code\[15\] = \{[^}]*\};", src).group(0),
        *move_tables(src),
        block(src, "static int first_key("),
        block(src, "static void current_binds("),
        *([block(src, "static unsigned stick_shows(")] if "static unsigned stick_shows(" in src else []),
        re.search(r"static unsigned rd16\(.*", src).group(0),
        re.search(r"static uint32_t rd32\(.*", src).group(0),
        re.search(r"static void wr16\(.*", src).group(0),
        block(src, "static unsigned cmap4_glyph("),
        block(src, "static int cmap4_point("),
        block(src, "static int cmap4_bind("),
        block(src, "static int bind_font("),
        "int main(int argc, char **argv) {",
        "    const char *m = getenv(\"MOVE_KEYS\");",
        "    if (m && sscanf(m, \"%d %d %d %d %d %d %d %d\", &move_key[0][0], &move_key[0][1], &move_key[1][0], &move_key[1][1],",
        "                     &move_key[2][0], &move_key[2][1], &move_key[3][0], &move_key[3][1]) != 8) return 2;",
        "    if (!strcmp(argv[1], \"binds\")) {",
        "        int bind[BINDS + 4] = {0};   /* (room for a snapshot without the movement keys) */",
        "        current_binds(bind);",
        "        for (int i = 0; i < BINDS; i++) printf(\"%d\\n\", bind[i]);",
        "        return 0;",
        "    }",
        "    FILE *f = fopen(argv[1], \"rb\");",
        "    static unsigned char st[65536];",
        "    const size_t n = fread(st, 1, sizeof st, f);",
        "    fclose(f);",
        "    if (argc > 2 && !strcmp(argv[2], \"font\")) { printf(\"%d\\n\", bind_font(st, n, 0)); return 0; }",
        "    printf(\"%d\\n\", cmap4_bind(st, n, atoi(argv[2])));",
        "    for (int i = 3; i < argc; i++) printf(\"%u\\n\", cmap4_glyph(st, n, (unsigned)strtoul(argv[i], NULL, 0)));",
        "    return 0;",
        "}", ""])
    with tempfile.TemporaryDirectory() as tmp:
        c, exe, bin_ = Path(tmp) / "bind.c", Path(tmp) / "bind", Path(tmp) / "cmap4.bin"
        c.write_text(harness, encoding="utf-8")
        bin_.write_bytes(sub)
        subprocess.run([a.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-I", str(ROOT), str(c), "-o", str(exe)], check=True)
        def run(f, move):
            return subprocess.run([str(exe), str(bin_), str(f)] + [str(x) for x in codes], check=True, capture_output=True,
                                  text=True, env=move_env(move)).stdout.split()

        outs = [run(f, WASD) for f in range(nfaces)]
        moved = {name: [run(f, move) for f in (0, KEYBOARD)] for name, move in MOVES.items()}
        binds = {name: subprocess.run([str(exe), "binds"], check=True, capture_output=True, text=True,
                                      env=move_env(move)).stdout.split() for name, move in MOVES.items()}
        # (a font whose cmap names a subtable at 0xFFFFFFF4: offset + 14 wrapped, and the check read 4 GB past the font)
        font = Path(tmp) / "wrap.otf"
        cmap_table = (0).to_bytes(2, "big") + (1).to_bytes(2, "big") + bytes(4) + (0xFFFFFFF4).to_bytes(4, "big")
        font.write_bytes(bytes(4) + (1).to_bytes(2, "big") + bytes(6) + b"cmap" + bytes(4) + (28).to_bytes(4, "big") +
                         len(cmap_table).to_bytes(4, "big") + cmap_table)
        wrapped = subprocess.run([str(exe), str(font), "font"], capture_output=True, text=True)
        check(wrapped.returncode == 0 and wrapped.stdout.split() == ["0"],
              "a cmap subtable offset that wraps: exit %d, %r" % (wrapped.returncode, wrapped.stdout))
    pads_of = {p: i for i, p in enumerate(P.PADS)}
    nsw, nst = len(P.SWITCH), len(P.STATIC)
    for f, out in enumerate(outs):
        done, got = int(out[0]), [int(x) for x in out[1:]]
        keyed = {b: first_key(b) for b in range(nsw) if f == KEYBOARD and first_key(b) in KEY_ICONS}
        nart = len(keyed) if f == KEYBOARD else sum(1 for b in BIND if ART(b))
        check(done == nart, "face %d: %d buttons bound, not %d" % (f, done, nart))
        for b, code in enumerate(P.SWITCH):
            want = 900 + keyed[b] if b in keyed else 200 + 100 * f + (BIND[b] if ART(b) else pads_of[P.DEFAULT[b]])
            check(got[b] == want, "face %d: Switch button %d (U+%04X): glyph %d, not %d" % (f, b, code, got[b], want))
        for i in range(len(stick_codes)):
            want = stick_glyph(i, WASD) if f == KEYBOARD else 1300 + i
            check(got[nsw + nst + P.FACE_CODES * nfaces + i] == want, "face %d: stick icon %d not on its glyph" % (f, i))
        for k, code in enumerate(P.STATIC):
            check(got[nsw + k] == 290 + 100 * f + k, "face %d: d-pad icon U+%04X: glyph %d" % (f, code, got[nsw + k]))
        for c in range(P.FACE_CODES * nfaces):
            want = mapping.get(P.PAD_BASE + c, 0)
            if P.PAD_BASE + c in stick_codes:
                continue
            check(got[nsw + nst + c] == want, "face %d: face icon U+%04X moved" % (f, P.PAD_BASE + c))
    at = 1 + nsw + nst + P.FACE_CODES * nfaces   # (the sticks' glyphs in a run's output)
    for name, (pad, keyboard) in moved.items():
        for i in range(len(stick_codes)):
            got, want = int(keyboard[at + i]), stick_glyph(i, MOVES[name])
            check(got == want, "the keyboard face, %s: stick icon %d (U+%04X, %s) on glyph %d, not %d" %
                  (name, i, stick_codes[i], P.STICKS[i][2], got, want))
            check(int(pad[at + i]) == 1300 + i, "a pad's face, %s: stick icon %d not the game's own" % (name, i))
        check(keyboard[:at] == outs[KEYBOARD][:at], "the keyboard face, %s: a button's or a face's icon moved" % name)
    # (the controls as the prompts follow them, current_binds: a movement key changed is a change, and none of the buttons')
    nbuttons = 2 * nsw
    for name, move in MOVES.items():
        check(len(binds[name]) >= nbuttons + 4 and binds[name][:nbuttons] == binds["W A S D"][:nbuttons] and
              [int(x) for x in binds[name][nbuttons:nbuttons + 4]] == [k[0] or k[1] for k in move],
              "current_binds, %s: %s after the buttons, not the movement keys' first keys" % (name, binds[name][nbuttons:]))
    check(len({tuple(b) for b in binds.values()}) == len({tuple(k[0] or k[1] for k in m) for m in MOVES.values()}),
          "current_binds does not tell the movement keys' changes apart")
    ways = re.search(r"stick_ways\[7\] = \{([^}]*)\}", src)
    moves = [(c, w) for c, stick, w in P.STICKS if stick == "move"]
    check([stick for _, stick, _ in P.STICKS[:len(moves)]] == ["move"] * len(moves) and len(moves) == 7,
          "build-assets' keys.rs STICKS: the left stick's icons are not the first seven (host/prompts.c stick_shows)")
    check(ways is not None and [int(x) for x in ways.group(1).split(",")] == [sum(1 << WAYS.index(w) for w in ws) for _, ws in moves],
          "host/prompts.c stick_ways is not the ways of build-assets' keys.rs STICKS")
    defaults = re.search(r"move_default\[4\] = \{([^}]*)\}", src)
    check(defaults is not None and [int(x, 16) for x in defaults.group(1).split(",")] == [P.MOVE[w] for w in WAYS],
          "host/prompts.c move_default is not build-assets' keys.rs MOVE")
    texts = check_text_code(a.cc, src, nfaces)
    print("ok: on each of %d faces, %d of %d Switch buttons pointed at the face's pad icons (a paddle at its default's), the "
          "rest at their defaults', the d-pad at its own; the faces' icons kept; the keyboard's left stick icons on %d sets of "
          "movement keys; %d text codes rewritten" % (nfaces, len(BIND), len(P.SWITCH), len(MOVES), texts))


def stick_glyph(i, move):
    """stick icon i's glyph in the test's cmap on the keyboard face with these movement keys"""
    kind, v = stick_shows(i, move)
    ways = P.STICKS[i][2]
    if kind == "keys" and P.STICKS[i][1] == "move" and len(ways) == 1:
        return 900 + P.MOVE[ways]   # (the build's picture of one way: its default key's icon)
    return {"key": 900, "keys": 1200, "game": 1300}[kind] + v


def move_tables(src):
    """host/prompts.c's tables for the movement keys (a source without them: none)"""
    found = [re.search(r"static const unsigned char %s\[\d\] = \{[^}]*\};" % n, src) for n in ("stick_ways", "move_default")]
    return [m.group(0) for m in found if m]


def check_text_code(cc, src, nfaces):
    """text_code on every code it rewrites (the game's, each face's bound and d-pad codes) and some it leaves, for each face"""
    base = re.search(r"#define BIND_BASE (0x[0-9A-Fa-f]+)", src)
    check(base is not None and int(base.group(1), 16) == P.BIND_BASE,
          "host/prompts.c BIND_BASE is not build-assets BIND_BASE")
    harness = "\n".join([
        "#include <stdio.h>", "#include <stdlib.h>", "#include <string.h>", '#include "gpu/vk/vk_api.h"',
        "#define PAD_BASE 0x%X" % P.PAD_BASE, "#define FACE_CODES %d" % P.FACE_CODES,
        "#define STATIC_AT %d" % P.STATIC_AT, base.group(0), *defines(src),
        "static int keyboard_built = 1;",
        # (key_slot as host/prompts.c's, without the font: each button and key a code of its own, in the order they come; a key
        # the font has no icon for gets none)
        "static unsigned char slot_button[SLOTS], slot_key[SLOTS];",
        "static int nslots;",
        "static int key_slot(int b, int k) {",
        "    int s = 0;",
        "    if (%s) return -1;" % " && ".join("k != %d" % k for k in KEY_ICONS),
        "    while (s < nslots && !(slot_button[s] == b && slot_key[s] == k)) s++;",
        "    if (s == nslots) slot_button[s] = (unsigned char)b, slot_key[s] = (unsigned char)k, nslots++;",
        "    return SLOT_BASE + s;",
        "}",
        re.search(r"static const uint16_t stick_code\[15\] = \{[^}]*\};", src).group(0),
        re.search(r"static const uint16_t switch_code\[HOS_SW_N\] = \{[^}]*\};", src).group(0),
        re.search(r"static const unsigned char switch_pad\[HOS_SW_N\] = \{[^}]*\};", src).group(0),
        re.search(r"static const uint16_t static_code\[3\] = \{[^}]*\};", src).group(0),
        *move_tables(src),
        *([block(src, "static unsigned stick_shows(")] if "static unsigned stick_shows(" in src else []),
        block(src, "static int text_code("),
        # (the arguments in their order: "face N" and "move U D L R" (the movement keys' first keys) from there on, else a code)
        "int main(int argc, char **argv) {",
        "    int bind[BINDS + 4], face = 0;   /* (room for the movement keys in a source without them) */",
        "    for (int b = 0; b < HOS_SW_N; b++) bind[b] = -1, bind[HOS_SW_N + b] = 0;",
        *("    bind[%d] = %d;" % kv for kv in BIND.items()),
        *("    bind[HOS_SW_N + %d] = %d;" % (b, first_key(b)) for b in KEYS),
        "    for (int i = 1; i < argc; i++) {",
        "        if (!strcmp(argv[i], \"face\")) face = atoi(argv[++i]);",
        "        else if (!strcmp(argv[i], \"move\")) for (int m = 0; m < 4; m++) bind[2 * HOS_SW_N + m] = atoi(argv[++i]);",
        "        else printf(\"%d\\n\", text_code((unsigned)strtoul(argv[i], NULL, 0), face, bind));",
        "    }",
        "    return 0;",
        "}", ""])
    nsw, npad = len(P.SWITCH), len(P.PADS)
    pads_of = {p: i for i, p in enumerate(P.PADS)}
    codes, of = [], []   # (each code, and what it shows: ("b", Switch button), ("k", d-pad icon) or None)
    for b, c in enumerate(P.SWITCH):
        codes.append(c), of.append(("b", b))
    for k, c in enumerate(P.STATIC):
        codes.append(c), of.append(("k", k))
    for f in range(nfaces):
        for b in range(nsw):
            for p in (0, npad - 1, b % npad):
                codes.append(P.BIND_BASE + (f * nsw + b) * npad + p), of.append(("b", b))
        for k in range(len(P.STATIC)):
            codes.append(P.PAD_BASE + P.FACE_CODES * f + P.STATIC_AT + k), of.append(("k", k))
        codes.append(P.PAD_BASE + P.FACE_CODES * f), of.append(None)   # (a face's pad icon: not a text's)
    stick_codes = [c for c, _, _ in P.STICKS]
    for i, c in enumerate(stick_codes):   # (a stick's: the game's code, or the keyboard's or the game's own after a change)
        for code in (c, P.STICK_KEYS + i, P.STICK_GAME + i):
            codes.append(code), of.append(("s", i))
    for c in (0x41, 0xE048, 0xE04A, 0xE090, P.BIND_BASE - 1, P.BIND_BASE + nfaces * nsw * npad,
              P.SLOT_BASE + P.SLOTS - 1, P.STICK_GAME + len(stick_codes)):
        codes.append(c), of.append(None)   # (a slot not given out is no button's)
    with tempfile.TemporaryDirectory() as tmp:
        c, exe = Path(tmp) / "text.c", Path(tmp) / "text"
        c.write_text(harness, encoding="utf-8")
        subprocess.run([cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-I", str(ROOT), str(c), "-o", str(exe)], check=True)
        def run(*args):
            return [int(x) for x in subprocess.run([str(exe)] + [str(x) for x in args], check=True, capture_output=True,
                                                   text=True).stdout.split()]

        def stick_code(f, i, move, slots):
            """the code stick icon i takes in a text on face f with these movement keys (slots: the codes given out so far)"""
            kind, v = stick_shows(i, move) if f == KEYBOARD else ("game", i)
            if kind != "key":
                return (P.STICK_KEYS if kind == "keys" else P.STICK_GAME) + v
            pair = (nsw + i, v)   # (a stick on a key: a code of its own, as a button's)
            if pair not in slots:
                slots.append(pair)
            return P.SLOT_BASE + slots.index(pair)

        for f, (name, move) in ((f, m) for f in range(nfaces) for m in MOVES.items()):
            first = [k[0] or k[1] for k in move]
            got = run("face", f, "move", *first, *codes)
            slots = []   # (the keyboard face's codes for a button or a stick on a key, in the order they come)
            for code, what, g in zip(codes, of, got):
                if what is None:
                    want = -1
                elif what[0] == "k":
                    want = P.PAD_BASE + P.FACE_CODES * f + P.STATIC_AT + what[1]
                elif what[0] == "s":
                    want = stick_code(f, what[1], move, slots)
                elif f == KEYBOARD and first_key(what[1]) in KEY_ICONS:
                    pair = (what[1], first_key(what[1]))
                    if pair not in slots:
                        slots.append(pair)
                    want = P.SLOT_BASE + slots.index(pair)
                else:
                    b = what[1]
                    pad = BIND[b] if ART(b) else pads_of[P.DEFAULT[b]]
                    want = P.BIND_BASE + (f * nsw + b) * npad + pad
                check(g == want, "face %d, %s: text code U+%04X became %d, not %d" % (f, name, code, g, want))
        # (the up icon's code in one text through changes in one run: on W the build's, on I a code of its own, the same one
        # while it stays on I, on E another, on W the build's again; a code given out names its stick on a pad's face too)
        up, slot = stick_codes[1], P.SLOT_BASE
        rest = [k[0] for k in WASD[1:]]
        got = run("face", KEYBOARD, "move", 0x57, *rest, up, "move", 0x49, *rest, P.STICK_KEYS + 1, slot,
                  "move", 0x45, *rest, slot, "move", 0x57, *rest, slot + 1, "face", 0, slot, slot + 1)
        want = [P.STICK_KEYS + 1, slot, slot, slot + 1, P.STICK_KEYS + 1, P.STICK_GAME + 1,
                P.STICK_GAME + 1]
        check(got == want, "the up icon in a text, on W, I, I, E, W, then a pad's face: codes %s, not %s" % (got, want))
        # (a code given out names its button: on another face it becomes that face's code for the button)
        out = run("face", 0, P.SLOT_BASE)
        check(out == [-1], "a slot not given out in this run named a button: %r" % out)
    return len(codes)


def defines(src):
    """host/prompts.c's keyboard face's defines (KEY_BASE: past every face's code points) and BINDS (BUTTON_BINDS: a source
    without the movement keys has none)"""
    out = []
    for name in ("KEY_BASE", "SLOT_BASE", "SLOTS", "STICK_KEYS", "STICK_GAME", "BUTTON_BINDS", "BINDS"):
        d = re.search(r"#define %s .*" % name, src)
        check(d is not None or name == "BUTTON_BINDS", "host/prompts.c has no %s" % name)
        if d:
            out.append(d.group(0).split("/*")[0].rstrip())
    return out


if __name__ == "__main__":
    main()
