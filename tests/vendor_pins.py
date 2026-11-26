"""The vendor files longshot-builder downloads (builder/crates/build-driver/src/runtimes.rs), read from its source: for the
checks that scripts/fetch.py, a developer's tool, fetches the same files at the same commits as a player's build, so
the license the launchers show (they name the same commits) is the one for what is downloaded. Standard library only.

    repo, commit, files = builder_pins("FSR")   # files: [(path in the repository, SHA-256, name in the game folder)]
"""
import re
from pathlib import Path

RUNTIMES = Path(__file__).resolve().parents[1] / "builder" / "crates" / "build-driver" / "src" / "runtimes.rs"


def builder_pins(name):
    """(repo, commit, files) of <name>_REPO and <name>_FILES in runtimes.rs"""
    text = RUNTIMES.read_text(encoding="utf-8")
    m = re.search(r'const %s_REPO: \(&str, &str\) = \("([^"]+)", "([0-9a-f]{40})"\);' % name, text)
    if not m:
        raise SystemExit("FAIL: %s has no %s_REPO" % (RUNTIMES, name))
    body = re.search(r"const %s_FILES: &\[File\] = &\[(.*?)\n\];" % name, text, re.S)
    if not body:
        raise SystemExit("FAIL: %s has no %s_FILES" % (RUNTIMES, name))
    files = re.findall(r'f\(\s*"([^"]+)",\s*"([0-9a-f]{64})",\s*"([^"]+)",?\s*\)', body.group(1))
    return m.group(1), m.group(2), files
