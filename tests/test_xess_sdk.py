"""scripts/fetch.py xess (Intel's XeSS library): its file table (the commit third_party/xess's headers come from), the
SHA-256 checks, the retries, the copy into a game folder, a player's own libxess.dll left alone, Intel's license accepted
first (here, and in longshot-builder: the same files), without the network.

Run with: python -I tests/test_xess_sdk.py
"""
import hashlib
import importlib.util
import io
import os
import sys
from pathlib import Path
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("fetch_under_test", ROOT / "scripts" / "fetch.py")
fetch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fetch)
xess = fetch.XESS


def digest(data):
    return hashlib.sha256(data).hexdigest()


class Opener:
    """urlopen over a dict of path -> bytes; records what was asked for. fail: paths that raise OSError once each"""
    def __init__(self, contents, fail=()):
        self.contents, self.asked, self.fail = contents, [], set(fail)

    def __call__(self, url, timeout=None):
        if timeout != fetch.TIMEOUT:
            raise AssertionError("urlopen without the timeout (%r)" % (timeout,))
        path = url[len(xess.base) + 1:]
        self.asked.append(path)
        if path in self.fail:
            self.fail.discard(path)
            raise OSError("connection reset")
        return io.BytesIO(self.contents[path])


def fake_table(data):
    """FILES with each file's SHA-256 that of data[path]"""
    return [(p, digest(data[p]), name) for p, _, name in xess.files]


class TableTests(unittest.TestCase):
    def test_pinned_commit_is_the_headers(self):
        # (the library from the same commit as the API headers the renderer is compiled against)
        self.assertRegex(xess.commit, r"^[0-9a-f]{40}$")
        self.assertIn(xess.commit, xess.base)
        self.assertIn(xess.commit, xess.license_url)
        provenance = (ROOT / "third_party" / "xess" / "PROVENANCE.md").read_text(encoding="utf-8")
        self.assertIn("commit `%s`" % xess.commit, provenance)

    def test_files(self):
        names = [name for _, _, name in xess.files]
        self.assertEqual(names, ["libxess.dll", "LICENSE-INTEL-XESS-SDK.txt", "THIRD-PARTY-INTEL-XESS-SDK.txt"])
        self.assertEqual(xess.files[0][0], "bin/" + xess.dlls()[0])
        for _, sha, _ in xess.files:
            self.assertRegex(sha, r"^[0-9a-f]{64}$")

    def test_the_name_the_game_loads(self):
        # (gpu/vk/vk_xess.cpp kLibrary)
        source = (ROOT / "gpu" / "vk" / "vk_xess.cpp").read_text(encoding="utf-8")
        self.assertIn('kLibrary = "%s";' % xess.dlls()[0], source)


class FetchTests(unittest.TestCase):
    def setUp(self):
        self.data = {p: ("file %s" % p).encode() for p, _, _ in xess.files}

    def test_fetch_and_install(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(xess, "files", fake_table(self.data)):
            opener = Opener(self.data)
            said = []
            xess.fetch(os.path.join(tmp, "sdk"), opener=opener, say=said.append)
            self.assertEqual(len(said), 3)
            self.assertTrue(xess.complete(os.path.join(tmp, "sdk")))
            xess.fetch(os.path.join(tmp, "sdk"), opener=opener, say=said.append)   # (there already: nothing asked)
            self.assertEqual(len(opener.asked), 3)
            game = xess.install(os.path.join(tmp, "sdk"), os.path.join(tmp, "game"))
            self.assertTrue(xess.complete(game))

    def test_wrong_file_not_kept(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(xess, "files", fake_table(self.data)):
            bad = dict(self.data)
            first = xess.files[0][0]
            bad[first] = b"something else"
            with self.assertRaises(ValueError):
                xess.fetch(tmp, opener=Opener(bad), say=lambda _: None)
            self.assertFalse(os.path.exists(os.path.join(tmp, xess.files[0][2])))
            self.assertFalse(os.path.exists(os.path.join(tmp, xess.files[0][2] + ".part")))
            self.assertFalse(xess.complete(tmp))

    def test_players_dll_left_alone(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(xess, "files", fake_table(self.data)):
            xess.fetch(os.path.join(tmp, "sdk"), opener=Opener(self.data), say=lambda _: None)
            game = os.path.join(tmp, "game")
            os.makedirs(game)
            theirs = os.path.join(game, xess.dlls()[0])
            with open(theirs, "wb") as f:
                f.write(b"a newer SDK's libxess.dll")
            said = []
            xess.install(os.path.join(tmp, "sdk"), game, say=said.append)
            with open(theirs, "rb") as f:
                self.assertEqual(f.read(), b"a newer SDK's libxess.dll")
            self.assertEqual(sorted(os.listdir(game)), [xess.dlls()[0]])   # (nothing of the pinned set beside it)
            self.assertEqual(len(said), 1)
            self.assertIn(xess.dlls()[0], said[0])

    def test_retry_after_a_reset(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(xess, "files", fake_table(self.data)):
            second = xess.files[1][0]
            opener = Opener(self.data, fail=[second])
            xess.fetch(tmp, opener=opener, say=lambda _: None)
            self.assertEqual(opener.asked.count(second), 2)
            self.assertTrue(xess.complete(tmp))


class LicenseTests(unittest.TestCase):
    """Intel's license accepted before anything of Intel's is downloaded: --accept-intel-license, as AMD's and NVIDIA's"""
    def test_accepted(self):
        self.assertTrue(xess.accepted(True, interactive=False))
        self.assertFalse(xess.accepted(False, interactive=False))   # (no one to ask: no)
        self.assertTrue(xess.accepted(False, ask=lambda _: " Yes ", interactive=True))
        self.assertFalse(xess.accepted(False, ask=lambda _: "no", interactive=True))

        def eof(_):
            raise EOFError
        self.assertFalse(xess.accepted(False, ask=eof, interactive=True))

    def test_script_downloads_nothing_unaccepted(self):
        with mock.patch.object(xess, "fetch", side_effect=AssertionError("downloaded without the license")), \
                mock.patch.object(xess, "accepted", return_value=False), \
                mock.patch("sys.stdout", io.StringIO()), mock.patch("sys.stderr", io.StringIO()) as err:
            self.assertEqual(fetch.main(["xess", "--game", "unused"]), 2)
        self.assertIn("--accept-intel-license", err.getvalue())


class BuilderTests(unittest.TestCase):
    """longshot-builder downloads the same files at the same commit for a player's build (runtimes.rs, which also refuses
    to download before the license is accepted, --accept-licenses, and leaves a player's own DLL alone: its own tests)"""
    def test_the_builder_pins_the_same_files(self):
        sys.path.insert(0, str(ROOT / "tests"))
        from vendor_pins import builder_pins
        repo, commit, files = builder_pins("XESS")
        self.assertEqual(commit, xess.commit)
        self.assertIn(repo, xess.base)
        self.assertEqual([tuple(f) for f in files], [tuple(f) for f in xess.files])


if __name__ == "__main__":
    unittest.main()
