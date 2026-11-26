"""Two switches left over from past measurements are gone, their default kept for everyone: RX_SOFT_WRITE_WATCH (=0 built
with Windows' write watch instead of the soft one, builder/rt/a64.h A64_MARK) and LONGSHOT_OCCLUSION_OFF (=1 made the game
skip its occlusion queries, a text patch of the old lift). Nothing reads either any more: the soft write watch is always built in, the
guest window is never write-watched, and the game makes its occlusion queries."""
import os
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
ROOTS = ("host", "game", "gpu", "builder/crates", "builder/rt", "scripts", "docs", ".forgejo", "README.md", "CONTRIBUTING.md",
         "AGENTS.md")
SKIP = {"third_party", "build", "gen", "work", "target", "node_modules", "__pycache__"}


def sources():
    for top in ROOTS:
        top = ROOT / top
        if top.is_file():
            yield top
            continue
        for base, dirs, files in os.walk(top):
            dirs[:] = [d for d in dirs if d not in SKIP]
            for name in files:
                if name.endswith((".py", ".c", ".h", ".cpp", ".rs", ".md", ".yml", ".sh", ".ps1", ".cs")):
                    yield Path(base) / name


class SwitchesRemoved(unittest.TestCase):
    def test_neither_switch_is_read(self):
        for path in sources():
            text = path.read_text(encoding="utf-8", errors="replace")
            for switch in ("RX_SOFT_WRITE_WATCH", "LONGSHOT_OCCLUSION_OFF", "hos_occlusion_off"):
                self.assertNotIn(switch, text, "%s: %s" % (path.relative_to(ROOT), switch))

    def test_the_defaults_stay(self):
        # (the lift's a64_config.h always sets A64_WRITE_TRACK 1: lift's tests/config_h.rs)
        self.assertNotIn("MEM_WRITE_WATCH", (ROOT / "host" / "hos_main.c").read_text(encoding="utf-8"))


if __name__ == "__main__":
    os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")
    unittest.main()
