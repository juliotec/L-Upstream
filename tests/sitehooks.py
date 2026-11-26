"""Hook sites as the lift lowers them, for the site tests: `longshot-builder dev site-hooks` (builder/crates/longshot-builder
src/dev/site_hooks.rs) lifts each named site in a function of its own and wraps it in `int <name>(Ctx *c)`; clang
compiles that IR into an object the test links with its fixture. The hook a test runs is the game's (the flag test, the
call, a replaced call's X30 and callee, a rule inline), not a copy of it. Standard library only.

    sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
    import sitehooks

    sitehooks.use(args.cc, args.cargo)   # (the fixture's compiler and cargo, as the test was given them)
    src = "... void f(Ctx *c) {\n" + sitehooks.line(x) + "\n ... }"   # (x: a site, tests/sitecheck.py's site() or load())
    subprocess.run([args.cc, ..., src_path, *sitehooks.objects(tmp, src), "-o", exe], check=True)

line(x, pc=None, callee=None, call=None): the C at site x's place in a fixture's function (the context is c), calling a wrapper of
its own, declared in the line: an entry that can return returns on the answer; a branch or a jump runs GOTO (the text
where the fixture puts the transfer to its target) on it; an allow runs THEN (its own block) when COND (its own test)
holds and the hook says yes (off: the branch as the game has it); a replace opens the block whose body is the
instruction's own C, which REPLACE_END closes: the instruction runs when the site is off or its callback was not run.
Every other form, and a rule, is called. pc puts the site's function at the game's address (a replaced call's X30 is then
pc + 4, as the bl would set it); callee is the function a replaced bl calls, which the hook tells the callback in
hos_site_callee; call=True makes a replace a call with no callee told (a blr's: X30 set, hos_site_callee left).
A callee is a call, and a pc alone is not. objects(tmp, *texts): the object of the wrappers the texts name, made in tmp (a folder): [its path], or
[] when they name none; SystemExit("FAIL: ...") when the builder or clang does not make it.

The fixture defines what the hooks refer to with external linkage: `volatile uint8_t hos_site_on_<ident>`,
`int (*volatile hos_site_fn_<ident>)(Ctx *)` (tests/sitecheck.py's slot()), `_Thread_local uint64_t hos_site_callee` (a
replaced call) and a rule's value. Its Ctx must have the lifted code's layout up to the fields the hook touches
(builder/rt/a64.h: X30 at 240 for a replaced call, the vector registers at 272 for a rule). The object has native TLS,
which mingw's gcc (emulated TLS) does not link with: a test compiles its fixture with clang (its --cc, run_all's {clang}).
"""
import itertools
import os
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BUILD = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348"   # (the supported dump's build: the one table)
GOTO = "{goto}"   # (a branch's or a jump's text: where the fixture puts the transfer to the target)
COND = "{cond}"   # (an allow's text: where the fixture puts the branch's own test)
THEN = "{then}"   # (an allow's text: the branch's own block, its transfer to its target)
REPLACE_END = "    } }"   # (closes a replace site's block, after the instruction's own code)

CARGO = None   # (use(): the cargo and the clang the test was given; else PATH's, and clang_program()'s)
CLANG = None
SPECS = {}   # (each wrapper line() made: its name -> its spec for the builder)
COUNT = itertools.count()


def clang_program(clang=None):
    """clang, as given, else the checkout's tools/llvm one (tools.lock), else PATH's"""
    if clang:
        return clang
    own = ROOT / "tools" / "llvm" / "bin" / ("clang.exe" if os.name == "nt" else "clang")
    return str(own) if own.is_file() else shutil.which("clang") or shutil.which("clang-23")


def use(cc=None, cargo=None):
    """The test's own tools: cc (its fixture's compiler) compiles the hooks' IR too when it is a clang, else
    clang_program() does; cargo runs the builder"""
    global CARGO, CLANG
    CARGO = cargo
    CLANG = cc if cc and "clang" in os.path.basename(cc) else None


def spec(x, name, pc=None, callee=None, call=None):
    """The builder's spec of wrapper name for site x: <name>=<site>[@<pc>[:[<callee>]]]"""
    call = callee is not None if call is None else call
    at = "" if pc is None else "@%#x" % pc + (":" if call else "") + ("" if callee is None else "%#x" % callee)
    return "%s=%s%s" % (name, x.name, at)


def line(x, pc=None, callee=None, call=None):
    """The C of site x's hook at its place in a fixture's function (see the docstring)"""
    if (callee is not None or call) and pc is None:
        raise ValueError("%s: a call needs its pc" % x.name)
    fn = "hook_%s_%d" % (x.ident, next(COUNT))
    SPECS[fn] = spec(x, fn, pc, callee, call)
    d, note = "int %s(Ctx *);" % fn, "   /* (site %s) */" % x.name
    if x.rule:
        return "    { %s %s(c); }   /* (rule %s) */" % (d, fn, x.name)
    if x.at in ("branch", "jump"):
        return "    { %s if (%s(c)) { %s } }%s" % (d, fn, GOTO, note)
    if x.at == "allow":
        return "{ %s if ((%s) && %s(c)) %s }%s" % (d, COND, fn, THEN, note)
    if x.at == "replace":
        return "    { %s if (!%s(c)) {%s" % (d, fn, note)
    if x.at == "entry" and x.can_return:
        return "    { %s if (%s(c)) return; }%s" % (d, fn, note)
    return "    { %s %s(c); }%s" % (d, fn, note)


def build(tmp, specs, cargo=None, clang=None, build_id=BUILD):
    """The object of these specs' wrappers, made in tmp (a folder): its path. Stops with SystemExit when the builder or
    clang does not make it"""
    tmp = Path(tmp)
    ll, obj = tmp / "site_hooks.ll", tmp / "site_hooks.o"
    cargo = cargo or CARGO or shutil.which("cargo") or "cargo"
    made = subprocess.run([cargo, "run", "-q", "-p", "longshot-builder", "--", "dev", "site-hooks", "--tables",
                           str(ROOT / "game"), "--build", build_id, "--out", str(ll), *specs],
                          cwd=ROOT, capture_output=True, text=True)
    if made.returncode:
        raise SystemExit("FAIL: longshot-builder dev site-hooks: " + made.stderr)
    cc = clang_program(clang or CLANG) or "clang"
    done = subprocess.run([cc, "-c", "-O1", "-Wno-override-module", str(ll), "-o", str(obj)], capture_output=True, text=True)
    if done.returncode:
        raise SystemExit("FAIL: clang on the hooks' IR: " + done.stderr)
    return str(obj)


def objects(tmp, *texts):
    """[the object of the wrappers line() made that the texts name], or [] when they name none (see the docstring)"""
    named = sorted(set(re.findall(r"\bhook_\w+", "".join(texts))) & set(SPECS))
    return [build(tmp, [SPECS[n] for n in named])] if named else []


def head_of(e):
    return "void f_%08x(Ctx *__restrict A64_NOESC c) {\n" % e


def splice_entry(text, e, hook):
    """text (a fixture's lifted C with f_<e> in it) with hook as the first lines of f_<e>'s body"""
    head = head_of(e)
    at = text.index(head) + len(head)
    return text[:at] + hook + "\n" + text[at:]
