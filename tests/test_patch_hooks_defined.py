"""Every host function the game's patches call (builder/crates/edits/src/patches.rs: the host functions each patch's code calls,
written into the lifted game) is defined in host/, game/ or gpu/vk/. #2617 took hos_disable_camera_centering out of host/mouse_native.c while the patch still called it: every
build of main then failed at the link (undefined symbol), which no source check saw.
python -I -B tests/test_patch_hooks_defined.py
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    # (each host function a patch calls is named there as a string: "hos_...")
    patches = (ROOT / "builder" / "crates" / "edits" / "src" / "patches.rs").read_text(encoding="utf-8")
    names = sorted(set(re.findall(r'"(hos_\w+)"', patches)))
    if not names:
        raise SystemExit("FAIL: no hos_ function named in builder/crates/edits/src/patches.rs")
    sources = []
    for d in ("host", "game", "gpu/vk"):
        for p in (ROOT / d).rglob("*"):
            if p.suffix in (".c", ".cpp") and p.is_file():
                sources.append(p.read_text(encoding="utf-8", errors="replace"))
    text = "\n".join(sources)
    missing = []
    for n in names:
        # (a definition: the name, its parameter list, then a body's brace, at the start of a line)
        if not re.search(r"^[\w \*]*\b" + n + r"\s*\([^;{]*\)\s*\{", text, re.M):
            missing.append(n)
    if missing:
        raise SystemExit("FAIL: called by the game's patches but defined nowhere in host/, game/ or gpu/vk/: " + ", ".join(missing))
    print("PASS: the %d host functions the game's patches call are all defined" % len(names))


if __name__ == "__main__":
    main()
