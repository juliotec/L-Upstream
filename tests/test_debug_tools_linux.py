"""Game-free: the OoT debug tools (cheats, pouch, flight, lift, free view) and the HOS_OOT_PROTECT_HEARTS switch are
compiled on Linux as well as Windows. They were inside `#ifdef _WIN32`, although everything they call exists on Linux
too (host/posix provides the Windows calls they use), so a Linux build had the Insert menu's cheats without their code.

The check follows game/oot_tools.c's and host/hos_main.c's preprocessor conditions to each tool's definition (and to the
switch's read, in game/oot_tools.c's game_init, and hos_main.c's call of it) and
evaluates them twice: with _WIN32 defined, and with __linux__ defined instead. Conditions on other macros count as true.

python -I -B tests/test_debug_tools_linux.py
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The debug tools' definitions (game/oot_tools.c), the line that reads the hearts switch (game/oot_tools.c game_init),
# and host/hos_main.c's call of game_init.
TOOLS = [
    ("game/oot_tools.c", r"static int oot_pouch_change\("),
    ("game/oot_tools.c", r"static void oot_update_player_cheats\("),
    ("game/oot_tools.c", r"int game_flight_shortcut_ready\(void\) \{"),
    ("game/oot_tools.c", r"static void oot_write_flight_rotation\("),
    ("game/oot_tools.c", r"void hos_oot_flight_rotation\("),
    ("game/oot_tools.c", r"void hos_oot_flight_pose\("),
    ("game/oot_tools.c", r"void hos_oot_lift_player\("),
    ("game/oot_tools.c", r"static void oot_build_free_view\("),
    ("game/oot_tools.c", r"void hos_oot_free_view\("),
    ("game/oot_tools.c", r'getenv\("HOS_OOT_PROTECT_HEARTS"\)'),
    ("host/hos_main.c", r"\bgame_init\(\);"),
]


def condition(expr, defined):
    """True, False or None (a condition on other macros) for one #if / #ifdef / #ifndef line."""
    m = re.match(r"#\s*ifdef\s+(\w+)", expr)
    if m:
        return m.group(1) in defined if m.group(1) in ("_WIN32", "__linux__") else None
    m = re.match(r"#\s*ifndef\s+(\w+)", expr)
    if m:
        return m.group(1) not in defined if m.group(1) in ("_WIN32", "__linux__") else None
    m = re.match(r"#\s*(?:el)?if\s+(.*)", expr)
    if not m:
        return None
    body = re.sub(r"/\*.*?\*/|//.*", "", m.group(1)).strip()
    names = re.findall(r"defined\s*\(\s*(\w+)\s*\)", body)
    if not names or any(n not in ("_WIN32", "__linux__") for n in names):
        return None
    rest = re.sub(r"defined\s*\(\s*\w+\s*\)", "", body).replace("||", "").replace("!", "").strip()
    if rest:
        return None
    if "&&" in body or "!" in body:
        return None
    return any(n in defined for n in names)


def compiled(lines, index, defined):
    """Whether line `index` is compiled with the macros in `defined` (unknown conditions count as true)."""
    stack = []   # per open #if: [a branch before was certainly taken, this branch's value (True, False, None)]
    for line in lines[:index]:
        s = line.strip()
        if re.match(r"#\s*if", s):
            v = condition(s, defined)
            stack.append([v is True, v])
        elif re.match(r"#\s*elif", s):
            v = False if stack[-1][0] else condition(s, defined)
            stack[-1] = [stack[-1][0] or v is True, v]
        elif re.match(r"#\s*else", s):
            v = stack[-1][1]
            stack[-1][1] = False if stack[-1][0] else (None if v is None else not v)
        elif re.match(r"#\s*endif", s):
            stack.pop()
    return all(v is not False for _, v in stack)


def first_statement(lines, index):
    """For a function definition, the first line of its body that is not a preprocessor line."""
    for j in range(index + 1, min(index + 20, len(lines))):
        if lines[j].strip() and not lines[j].lstrip().startswith("#"):
            return j
    return index


def main():
    problems = []
    for rel, pattern in TOOLS:
        lines = (ROOT / rel).read_text(encoding="utf-8").splitlines()
        hits = [i for i, line in enumerate(lines) if re.search(pattern, line)]
        if not hits:
            problems.append("not found in %s: %s" % (rel, pattern))
            continue
        for i in hits + [first_statement(lines, i) for i in hits if lines[i].rstrip().endswith("{")]:
            for system, defined in (("Windows", {"_WIN32"}), ("Linux", {"__linux__"})):
                if not compiled(lines, i, defined):
                    problems.append("%s:%d (%s) is not compiled on %s" % (rel, i + 1, lines[i].strip(), system))
    if problems:
        raise SystemExit("\n".join(problems))
    print("ok: the %d debug tools are compiled on Windows and Linux" % len(TOOLS))


if __name__ == "__main__":
    main()
