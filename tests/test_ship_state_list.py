"""The pipeline state list a release ships (merged from the sources in assets/pipeline_states/): the repository's sources read, and
every release path carries them. v2.0.8 to v2.0.14 shipped none (their app parts' tools/work and the Linux tools part's work/ held only the play
profile), and a new install's first field load was drawn white for seconds while its pipelines compiled in play.

  - the committed sources are xz files of whole lists of this build's format, and hold pipelines;
  - `longshot-builder dev release sources` carries them in what a release's longshot-builder carries
    (assets/pipeline_states/), and stops when there are none (build-driver's sources.rs:
    the_tree_carries_the_pipeline_state_lists_and_its_version).

Game-free: the list holds shader hashes and draw state, and the rest is synthetic.
    python -I -B tests/test_ship_state_list.py
"""
import importlib.util
import lzma
import shutil
import struct
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class ShippedList(unittest.TestCase):
    def setUp(self):
        self.tool = load("pipeline_states", ROOT / "scripts" / "pipeline_states.py")
        self.src = ROOT.joinpath(*self.tool.SOURCES)
        self.tmp = Path(tempfile.mkdtemp(prefix="longshot-ship-states-"))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def fake_list(self, n, magic=None):
        return lzma.compress((magic or self.tool.MAGIC) + struct.pack("<I", self.tool.SIZE) + b"".join(
            struct.pack("<Q", i + 1) + bytes(self.tool.SIZE - 8) for i in range(n)))

    def fake_sources(self, d, data):
        """d: a sources folder with one contributed list of these bytes, and its note"""
        c = d / self.tool.CONTRIB
        c.mkdir(parents=True)
        (c / ("someone" + self.tool.EXT)).write_bytes(data)
        try:
            n = len(self.tool.parse(lzma.decompress(data), "fake"))
        except (lzma.LZMAError, SystemExit):
            n = 0
        self.tool.write_note(str(c / "someone.md"), "someone", {k: "x" for k in self.tool.FIELDS}, [b""] * n)

    def test_committed_sources_read(self):
        self.assertTrue(self.src.is_dir(), "assets/pipeline_states/ is not in the repository")
        paths = self.tool.sources(str(self.src))
        self.assertTrue(paths)
        for p in paths:
            self.assertEqual(Path(p).read_bytes()[:6], self.tool.XZ_MAGIC, "%s is not xz-compressed" % p)
        recs = self.tool.check_sources(str(self.src))
        self.assertGreater(len(recs), 0)


if __name__ == "__main__":
    unittest.main()
