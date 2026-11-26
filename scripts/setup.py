"""Set up a clone for development, the part of docs/dev/setup.md that needs no account: run it as `mise run setup`, which
first installs, on Linux, the headers, glslang and Python packages the game-free checks use.
Safe to run again: each step does only what is missing, then a summary says what was done and what is left for you.

    - the team's Tooling repository beside this checkout (../Tooling, where every doc and skill runs it from): cloned when missing, else
      brought up to date (git pull --ff-only) when it is a git checkout on a branch; a jj repository or a detached HEAD
      is left alone
    - the pre-commit and commit-msg hooks (pre-commit install)
    - Claude Code's magicbean plugin, when Claude Code is installed: the commands to install it when it is missing
    - Rust, which a build compiles longshot-builder with
    - tools.lock's build tools in the checkout's tools/: `mise run tools` when they are missing (dev.py's builds install
      their own into the development folder)

It never signs in and never reads or writes a token: sign in yourself, `python -I ../Tooling/scripts/forge.py login`.
Standard library only.

    python -I scripts/setup.py
"""
import argparse
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOLING_URL = "https://git.magicbean.dev/MagicBeanDev/Tooling.git"
PLUGIN = "magicbean@magicbean"
SETUP_DOC = "docs/dev/setup.md"


def run(cmd, cwd=None, timeout=300):
    """A command's exit code and its output (stdout and stderr together), or (None, why) when it could not start"""
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0")   # (a public clone never needs a password: never wait for one)
    try:
        r = subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
    except (OSError, subprocess.TimeoutExpired) as e:
        return None, str(e)
    return r.returncode, r.stdout.strip()


def last_line(text):
    return text.strip().splitlines()[-1] if text.strip() else "no output"


def tooling_path():
    return (ROOT.parent / "Tooling").resolve()


def tooling(done, todo):
    """Clone Tooling when it is missing; fast-forward a git checkout on a branch; leave anything else as it is"""
    path = tooling_path()
    if not shutil.which("git"):
        todo.append("Tooling (%s): git is not installed; install it, then run setup again" % path)
        return
    if not path.exists():
        code, out = run(["git", "clone", TOOLING_URL, str(path)])
        if code == 0:
            done.append("Tooling: cloned into %s" % path)
        else:
            todo.append("Tooling: the clone into %s failed (%s): git clone %s %s" % (path, last_line(out), TOOLING_URL, path))
        return
    if (path / ".jj").exists():
        done.append("Tooling: %s is a jj repository, left alone (update it with jj)" % path)
        return
    if not (path / ".git").exists():
        todo.append("Tooling: %s is not a git checkout; move it away and run setup again" % path)
        return
    code, _ = run(["git", "-C", str(path), "symbolic-ref", "-q", "HEAD"])
    if code != 0:
        done.append("Tooling: %s has a detached HEAD, left alone" % path)
        return
    code, out = run(["git", "-C", str(path), "pull", "--ff-only"])
    if code == 0:
        done.append("Tooling: %s %s" % (path, "up to date" if "up to date" in out.lower() else "updated"))
    else:
        todo.append("Tooling: git -C %s pull --ff-only failed (%s)" % (path, last_line(out)))


def hooks(done, todo):
    """The hooks .pre-commit-config.yaml asks for (default_install_hook_types: pre-commit and commit-msg)"""
    if not (ROOT / ".git").exists():
        todo.append("git hooks: this checkout has no .git (a jj workspace without git?): run pre-commit install in a "
                    "colocated checkout")
        return
    if not shutil.which("pre-commit"):
        todo.append("git hooks: pre-commit is not on PATH; run mise install, then mise run setup")
        return
    code, out = run(["pre-commit", "install"], cwd=ROOT)
    if code == 0:
        done.append("git hooks: pre-commit and commit-msg installed")
    else:
        todo.append("git hooks: pre-commit install failed (%s)" % last_line(out))


def plugin(done, todo):
    """Claude Code's magicbean plugin; nothing to say when Claude Code is not installed. Installing it is yours to do"""
    claude = shutil.which("claude")
    if not claude:
        return
    code, out = run([claude, "plugin", "list"], timeout=60)
    if code != 0:
        todo.append("Claude Code plugin: claude plugin list failed (%s)" % last_line(out))
    elif PLUGIN in out:
        done.append("Claude Code plugin: %s installed" % PLUGIN)
    else:
        todo.append("Claude Code plugin: %s is missing; in Claude Code, type\n"
                    "      /plugin marketplace add %s\n"
                    "      /plugin install %s" % (PLUGIN, TOOLING_URL, PLUGIN))


RUST = "1.99"   # (mise.toml's: longshot-builder and the lifter, docs/dev/reference/builder.md)


def toolchain(done, todo):
    """What a game build needs here: Rust, to compile longshot-builder (dev.py does, and installs tools.lock's build tools
    itself, the ones players' builders use)"""
    cargo = shutil.which("cargo")
    if not cargo:
        todo.append("Rust: no cargo; `mise install` puts mise.toml's Rust %s on the PATH (%s)" % (RUST, SETUP_DOC))
        return
    code, out = run([cargo, "--version"], timeout=60)
    version = out.split()[1] if code == 0 and len(out.split()) > 1 else "?"
    if version.startswith(RUST + "."):
        done.append("Rust: cargo %s (longshot-builder; dev.py installs tools.lock's build tools)" % version)
    else:
        todo.append("Rust: cargo %s, not %s: `mise install` (mise.toml pins it) (%s)" % (version, RUST, SETUP_DOC))


def build_tools(done, todo):
    """tools.lock's build tools in the checkout (`mise run tools`: longshot-builder dev tools install, each tool's stamp in
    .tools/), which `longshot-builder --root .` compiles with as a player's build does"""
    installed = sorted(p.name for p in (ROOT / ".tools").glob("*")) if (ROOT / ".tools").is_dir() else []
    if installed:
        done.append("build tools: tools.lock's in tools/ (%s)" % ", ".join(installed))
    else:
        todo.append("build tools: `mise run tools` puts tools.lock's in tools/ (%s)" % SETUP_DOC)


def main():
    argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter).parse_args()
    done, todo = [], []
    for step in (tooling, hooks, plugin, toolchain, build_tools):
        step(done, todo)
    print("setup: done")
    for line in done:
        print("  - " + line)
    if todo:
        print("setup: left for you")
        for line in todo:
            print("  - " + line)
    print("Sign in once, if you have not: python -I %s login (%s)"
          % (os.path.relpath(tooling_path() / "scripts" / "forge.py", ROOT), SETUP_DOC))


if __name__ == "__main__":
    main()
