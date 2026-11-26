"""The line between the generic Switch emulation (host/, gpu/) and the game (game/), in the source, without a game.

- host/ and gpu/ include nothing of game/ but game/game.h (what the game gives them) and game/options.h (its option ids);
- game/ includes nothing of host/ or gpu/ but host/game_host.h (what the host gives the game) and the lift's runtime
  (a64.h, gen/rt: Ctx, the guest's registers and memory); a <...> system header Linux finds in host/posix (<windows.h>:
  the Win32 API's stand-in there) is the system's;
- no file of host/ or gpu/ names an Ocarina of Time symbol (an identifier oot_* or hos_oot_*) outside its comments and
  strings: the game's hooks have generic names in game/game.h;
- a host symbol game/ declares for itself (an extern or a prototype of a function or variable host/ defines) is declared
  by host/game_host.h too (or a header it includes): the header is all the game takes, no side door beside it.

An include, "..." or <...>, is followed as the build finds it (builder/crates/build-compile's plan): a "..." from the
file's own folder first, then the -I folders, the host's and the game's -I host/posix and -I host, the renderer's
-I host/posix and -I gpu/vk, and the lift's runtime; an include in a comment is none. Then the check is run over the tree
with a crossing planted, one at a time, and must fail on each: a game header included from host/nn_hid.c, an oot_ symbol
there, a game header reached through the renderer's -I gpu/vk, a host header reached with <...> from game/, and a host
symbol game/ declares that host/game_host.h does not. A check that passes everything is no check. Standard library only.

python -I -B tests/test_game_boundary.py
"""
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCES = (".c", ".cc", ".cpp", ".h", ".hh", ".hpp", ".inc", ".inl", ".def")
# (third-party code the renderer compiles: not Longshot's, and none of it is the game's)
NOT_OURS = ("gpu/vk/eden/", "gpu/vk/sirit/", "third_party/")
GAME_FOR_THE_HOST = {"game/game.h", "game/options.h"}
HOST_FOR_THE_GAME = {"host/game_host.h"}
RUNTIME = {"a64.h", "a64_config.h"}   # (gen/rt: the lift's runtime headers, builder/rt in the repository)
# (the Win32 API's headers on Linux, <windows.h> and its kin: host/posix stands in for the system's, which Windows' own
# headers are; a <...> include found there is the system's, not the host's)
POSIX = "host/posix/"
INCLUDE = re.compile(r'^[ \t]*#[ \t]*include[ \t]*(["<])([^">\n]+)[">]', re.M)
# (the -I folders, as the plan gives them: the host's and the game's, the renderer's)
INCLUDE_DIRS = {"host": ("host/posix", "host"), "game": ("host/posix", "host"), "gpu": ("host/posix", "gpu/vk")}
# (a file-level declaration of a function or variable of the host's: an extern, or a prototype; and a definition there)
DECLARED = re.compile(r"^[ \t]*(?:extern[ \t]+)?(?!return\b|else\b|static\b|typedef\b)(?:[A-Za-z_]\w*[ \t\*]+)+"
                      r"\**((?:hos|rx)_\w+)[ \t]*(?:\([^;{]*\)|\[[^\]]*\])?[ \t]*((?:,[ \t]*\**[ \t]*(?:hos|rx)_\w+[ \t]*)*);", re.M)
DEFINED = re.compile(r"^(?!static\b|extern\b|typedef\b)(?:[A-Za-z_]\w*[ \t\*]+)+\**((?:hos|rx)_\w+)[ \t]*"
                     r"(?:\([^;]*?\)\s*\{|(?:\[[^\]]*\])?[ \t]*(?:=[^;]*)?[;,])", re.M)
OOT = re.compile(r"\b(?:hos_)?oot_\w+")


def strip(text):
    """The C or C++ text without its comments and its string and character literals (lines kept)"""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("/*", i):
            j = text.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append("\n" * text.count("\n", i, j))
            i = j
        elif text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
        elif c in "\"'":
            j = i + 1
            while j < n and text[j] != c and text[j] != "\n":
                j += 2 if text[j] == "\\" else 1
            out.append(c + c)
            i = j + 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def files(tree, top):
    for dirpath, _, names in os.walk(tree / top):
        for name in sorted(names):
            p = Path(dirpath, name)
            rel = p.relative_to(tree).as_posix()
            if name.endswith(SOURCES) and not rel.startswith(NOT_OURS) and "/third_party/" not in rel:
                yield rel


def comments_out(text):
    """The text without its comments (lines kept; strings left, an include's among them)"""
    return re.sub(r"/\*.*?\*/|//[^\n]*", lambda m: "\n" * m.group(0).count("\n"), text, flags=re.S)


def includes(text):
    """(quote, name) of each include outside the comments"""
    return INCLUDE.findall(comments_out(text))


def resolve(tree, rel, inc, quote='"'):
    """Where #include "inc" (or <inc>) of the file rel is found, as a path from the tree's root, or None (a system or
    generated header)"""
    here = Path(rel).parent
    dirs = tuple(Path(d) for d in INCLUDE_DIRS.get(rel.split("/")[0], ()))
    for base in ((here,) if quote == '"' else ()) + dirs:
        cand = os.path.normpath((base / inc).as_posix()).replace(os.sep, "/")
        if (tree / cand).is_file():
            return cand
    if quote != '"':
        return None
    # (a path spelled into another top folder, there or not: where it points)
    spelled = os.path.normpath((here / inc).as_posix()).replace(os.sep, "/")
    return spelled if spelled.split("/")[0] in ("host", "game", "gpu") and spelled.split("/")[0] != rel.split("/")[0] else None


SEEN = {"files": 0, "includes": 0}


def problems(tree, read):
    """Every crossing of the line in the tree (read(rel) gives a file's text)"""
    out = []
    SEEN.update(files=0, includes=0)
    for top in ("host", "gpu"):
        for rel in files(tree, top):
            text = read(rel)
            SEEN["files"] += 1
            for quote, inc in includes(text):
                SEEN["includes"] += 1
                to = resolve(tree, rel, inc, quote)
                if to and to.startswith("game/") and to not in GAME_FOR_THE_HOST:
                    out.append('%s includes %s: the host and the renderer reach the game through game/game.h' % (rel, to))
            for name in sorted(set(OOT.findall(strip(text)))):
                out.append("%s names %s: a generic file names the game's hooks by game/game.h's names" % (rel, name))
    # (what host/game_host.h declares, with the headers of host/ and gpu/ it includes, followed; what host/ defines)
    given, todo = "", ["host/game_host.h"]
    seen = set(todo)
    while todo:
        h = todo.pop()
        text = comments_out(read(h))
        given += text
        for quote, inc in includes(text):
            to = resolve(tree, h, inc, quote)
            if to and to.startswith(("host/", "gpu/")) and to not in seen and (tree / to).is_file():
                seen.add(to)
                todo.append(to)
    given_names = set(re.findall(r"\b(?:hos|rx)_\w+\b", given))
    host_defined = set()
    for rel in files(tree, "host"):
        if rel.endswith((".c", ".cc", ".cpp")):
            text = comments_out(read(rel))
            for m in DEFINED.finditer(text):
                host_defined.add(m.group(1))
                if m.group(0).endswith(","):   # (the others a definition of several names gives: a, *b, c[2] = ...)
                    rest = text[m.end():text.find(";", m.end())]
                    host_defined.update(re.findall(r"(?:^|,)\s*\**\s*((?:hos|rx)_\w+)", rest))
    for rel in files(tree, "game"):
        SEEN["files"] += 1
        text = read(rel)
        for m in DECLARED.finditer(comments_out(text)):
            for name in [m.group(1)] + re.findall(r"(?:hos|rx)_\w+", m.group(2)):
                if name in host_defined and name not in given_names:
                    out.append("%s declares %s, the host's, which host/game_host.h does not give: a side door" % (rel, name))
        for quote, inc in includes(text):
            SEEN["includes"] += 1
            to = resolve(tree, rel, inc, quote)
            if to is None:
                if quote == '"' and inc not in RUNTIME:
                    out.append('%s includes "%s", which is neither the game\'s nor host/game_host.h' % (rel, inc))
            elif not to.startswith("game/") and to not in HOST_FOR_THE_GAME and not (quote == "<" and to.startswith(POSIX)):
                out.append("%s includes %s: the game reaches the host through host/game_host.h" % (rel, to))
    return out


def main():
    def read(rel):
        return (ROOT / rel).read_text(encoding="utf-8", errors="replace")

    found = problems(ROOT, read)
    if SEEN["files"] < 100 or SEEN["includes"] < 500:
        raise SystemExit("FAIL: only %(files)d files and %(includes)d includes read: the check reads the wrong tree" % SEEN)
    if found:
        raise SystemExit("FAIL: the game's line is crossed:\n  " + "\n  ".join(found))
    print("PASS: host/ and gpu/ reach game/ through game.h and options.h, game/ the host through game_host.h, "
          "and no generic file names an oot_ symbol (%(files)d files, %(includes)d includes)" % SEEN)

    # (the check must see a crossing: each planted alone, in memory)
    planted = (
        ("host/nn_hid.c", '#include "../game/oot_sprint.h"\n', "a game header's include"),
        ("host/nn_hid.c", "int oot_planted(void);\n", "an oot_ name"),
        ("gpu/vk/menu/audio.cpp", '#include "../../game/flags.h"\n', "a game header through the renderer's -I gpu/vk"),
        ("game/oot_menu.c", "#include <control.h>\n", "a host header through -I host, with <...>"),
        ("game/oot_menu.c", "extern const char *hos_romfs_dir;\n", "a host symbol host/game_host.h does not give"),
    )
    given = "".join(comments_out(read(h)) for h in ("host/game_host.h", "host/hos.h", "host/romfs.h"))
    if re.search(r"\bhos_romfs_dir\b", given):
        raise SystemExit("FAIL: the planted side door, hos_romfs_dir, is given by host/game_host.h now: plant another")
    for at, line, what in planted:
        def read_planted(rel, at=at, line=line):
            return line + read(rel) if rel == at else read(rel)
        if not [p for p in problems(ROOT, read_planted) if p.startswith(at)]:
            raise SystemExit("FAIL: %s planted in %s is not seen" % (what, at))
    print("PASS: each crossing planted is seen: " + ", ".join(what for _, _, what in planted))


if __name__ == "__main__":
    main()
