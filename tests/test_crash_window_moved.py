"""The crash window is the builder's (`longshot-builder play`, builder/crates/crash), not the game's: a source check, no
game and no compiler.

- The game starts no helper of itself and shows no window: no `--crash-watch` or `--crash-show`, no
  hos_crash_helper_main or hos_crash_watch_start, and host/hos_crash_dialog.c/.h, gpu/vk/vk_crash_window.cpp and
  gpu/vk/crash_window_api.h are gone, from the tree and from the build's lists.
- HOS_CRASH_WATCH is a GONE row of gpu/vk/switches.def; HOS_CRASH_TEST stays (host/hos_crash_test.c), armed once the
  crash handlers are set up.
- The game still writes its crash records (host/hos_crash_record.c), which the builder reads.

    python -I -B tests/test_crash_window_moved.py
"""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
GONE_FILES = ("host/hos_crash_dialog.c", "host/hos_crash_dialog.h", "gpu/vk/vk_crash_window.cpp",
              "gpu/vk/crash_window_api.h", "tests/test_crash_dialog.py")
GONE_NAMES = ("--crash-watch", "--crash-show", "hos_crash_helper_main", "hos_crash_watch_start", "hos_crash_dialog",
              "vk_crash_window", "crash_window_api", "mkwvk_crash_page")
SOURCES = ("host", "gpu/vk", "tests", "builder/crates/build-compile/src")
EXTS = (".c", ".h", ".cpp", ".hpp", ".rs", ".py", ".def", ".inc")


def main():
    problems = [f + " is still in the tree: the crash window is the builder's"
                for f in GONE_FILES if (ROOT / f).exists()]
    me = Path(__file__).resolve()
    for base in SOURCES:
        for path in sorted((ROOT / base).rglob("*")):
            if path.resolve() == me or not path.is_file() or path.suffix not in EXTS:
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            for name in GONE_NAMES:
                if name in text:
                    problems.append("%s names %s: the game has no crash helper or window any more"
                                    % (path.relative_to(ROOT).as_posix(), name))
    switches = (ROOT / "gpu/vk/switches.def").read_text(encoding="utf-8")
    if not re.search(r'^GONE\(HOS_CRASH_WATCH, ', switches, re.M):
        problems.append("gpu/vk/switches.def: HOS_CRASH_WATCH is not a GONE row (players may still set it)")
    if not re.search(r'^SW\(HOS_CRASH_TEST, ', switches, re.M):
        problems.append("gpu/vk/switches.def: HOS_CRASH_TEST lost its row")
    test = (ROOT / "host/hos_crash_test.c").read_text(encoding="utf-8")
    if "void hos_crash_test_arm(void) {" not in test or 'getenv("HOS_CRASH_TEST")' not in test:
        problems.append("host/hos_crash_test.c: no hos_crash_test_arm reading HOS_CRASH_TEST")
    main_c = (ROOT / "host/hos_main.c").read_text(encoding="utf-8")
    if not main_c.index("if (!reserve_guest()) {") < main_c.index("hos_crash_test_arm();"):
        problems.append("host/hos_main.c: HOS_CRASH_TEST is armed before the crash handlers are set up (reserve_guest)")
    if "hos_crash_record_start(" not in (ROOT / "host/mkw_glue.c").read_text(encoding="utf-8"):
        problems.append("host/mkw_glue.c: the game no longer starts its crash records, which the builder reads")
    if problems:
        raise SystemExit("FAIL:\n  " + "\n  ".join(problems))
    print("PASS: the game has no crash helper or window; HOS_CRASH_WATCH is gone, HOS_CRASH_TEST and the records stay")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
