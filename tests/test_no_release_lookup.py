"""The builder does not update itself (docs/dev/reference/releases.md): the Longshot Launcher reads Longshot's signed releases, and
installs and updates each version's longshot-builder. Nothing in it or the game asks a server for a release: no GitHub link,
no release lookup, no game update check (host/hos_update.c is gone), and none of the old self-update's hooks."""
import os
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]

# (the code that used to ask GitHub for a release or install a newer builder, and must not now)
SOURCES = ["host/hos_main.c", "game/oot_tools.c", "builder/crates/longshot-builder/src/main.rs", "builder/crates/build-driver/src/sources.rs"]


class NoLookup(unittest.TestCase):
    def test_no_builder_looks_for_a_release(self):
        for rel in SOURCES:
            text = (ROOT / rel).read_text(encoding="utf-8")
            for pattern in (r"api\.github\.com", r"github\.com/[^/\s\"]+/(Longshot|Hookshot)/releases", r"releases/latest",
                            r"NotRexed[/]Longshot", r"hos_update_check", r"LONGSHOT_LATEST_TAG", r"LONGSHOT_LINUX_LIST",
                            r"latest_tag", r"update.builder", r"--fetched", r"run_latest_setup"):
                self.assertIsNone(re.search(pattern, text), "%s: %s" % (rel, pattern))
        self.assertFalse((ROOT / "host" / "hos_update.c").exists(), "the game's own update check is back")

    def test_the_standalone_builders_are_gone(self):
        # (a release ships longshot-builder alone: no AppImage, no Windows builder or setup)
        # (recomplauncher.json stays: longshot-builder's own, for RecompLauncher's release zip)
        for gone in ("builder/LongshotBuilder", "builder/setup", "builder/linux", "scripts/linux/appimage.sh",
                     "scripts/builder_package.py"):
            self.assertFalse((ROOT / gone).exists(), gone)


if __name__ == "__main__":
    os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")
    unittest.main()
