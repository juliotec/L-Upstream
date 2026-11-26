"""scripts/fetch.py fsr (AMD's FSR runtime): its file table, the SHA-256 checks, the retries, the copy into a game folder
and AMD's license accepted first (here, and in longshot-builder: the same files), without the network.

Run with: python -I tests/test_ffx_sdk.py
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
fsr = fetch.FSR


def digest(data):
    return hashlib.sha256(data).hexdigest()


class Opener:
    """urlopen over a dict of path -> bytes; records what was asked for. fail: paths that raise OSError once each"""
    def __init__(self, contents, fail=()):
        self.contents, self.asked, self.fail = contents, [], set(fail)

    def __call__(self, url, timeout=None):
        if timeout != fetch.TIMEOUT:
            raise AssertionError("urlopen without the timeout (%r)" % (timeout,))
        path = url[len(fsr.base) + 1:]
        self.asked.append(path)
        if path in self.fail:
            self.fail.discard(path)
            raise OSError("connection reset")
        return io.BytesIO(self.contents[path])


def fake_table(data):
    """FILES with each file's SHA-256 that of data[path]"""
    return [(p, digest(data[p]), name) for p, _, name in fsr.files]


class TableTests(unittest.TestCase):
    def test_pinned_release(self):
        self.assertRegex(fsr.commit, r"^[0-9a-f]{40}$")
        self.assertIn(fsr.commit, fsr.base)

    def test_files(self):
        names = [name for _, _, name in fsr.files]
        self.assertEqual(names, ["amd_fidelityfx_loader_vk.dll", "amd_fidelityfx_upscaler_vk.dll", "LICENSE-AMD-FSR-SDK.md"])
        for path, sha, _ in fsr.files:
            self.assertTrue(path.startswith("Kits/FidelityFX/"), path)
            self.assertRegex(sha, r"^[0-9a-f]{64}$")


class FetchTests(unittest.TestCase):
    def setUp(self):
        self.data = {p: ("file %s" % p).encode() for p, _, _ in fsr.files}

    def test_fetch_and_install(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(fsr, "files", fake_table(self.data)):
            opener = Opener(self.data)
            said = []
            fsr.fetch(os.path.join(tmp, "sdk"), opener=opener, say=said.append)
            self.assertEqual(len(said), 3)
            self.assertTrue(fsr.complete(os.path.join(tmp, "sdk")))
            fsr.fetch(os.path.join(tmp, "sdk"), opener=opener, say=said.append)   # (there already: nothing asked)
            self.assertEqual(len(opener.asked), 3)
            game_ffx = fsr.install(os.path.join(tmp, "sdk"), os.path.join(tmp, "game", "ffx"))
            self.assertTrue(fsr.complete(game_ffx))

    def test_wrong_file_not_kept(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(fsr, "files", fake_table(self.data)):
            bad = dict(self.data)
            first = fsr.files[0][0]
            bad[first] = b"something else"
            with self.assertRaises(ValueError):
                fsr.fetch(tmp, opener=Opener(bad), say=lambda _: None)
            self.assertFalse(os.path.exists(os.path.join(tmp, fsr.files[0][2])))
            self.assertFalse(os.path.exists(os.path.join(tmp, fsr.files[0][2] + ".part")))
            self.assertFalse(fsr.complete(tmp))

    def test_players_dlls_left_alone(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(fsr, "files", fake_table(self.data)):
            fsr.fetch(os.path.join(tmp, "sdk"), opener=Opener(self.data), say=lambda _: None)
            game = os.path.join(tmp, "game")
            os.makedirs(game)
            theirs = os.path.join(game, fsr.dlls()[1])
            with open(theirs, "wb") as f:
                f.write(b"a newer SDK's upscaler")
            said = []
            fsr.install(os.path.join(tmp, "sdk"), game, say=said.append)
            with open(theirs, "rb") as f:
                self.assertEqual(f.read(), b"a newer SDK's upscaler")
            self.assertFalse(os.path.exists(os.path.join(game, fsr.dlls()[0])))   # (not mixed with v2.4.0's loader)
            self.assertEqual(len(said), 1)
            self.assertIn(fsr.dlls()[1], said[0])

    def test_retry_after_a_reset(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(fsr, "files", fake_table(self.data)):
            second = fsr.files[1][0]
            opener = Opener(self.data, fail=[second])
            fsr.fetch(tmp, opener=opener, say=lambda _: None)
            self.assertEqual(opener.asked.count(second), 2)
            self.assertTrue(fsr.complete(tmp))


class LicenseTests(unittest.TestCase):
    """AMD's license accepted before anything of AMD's is downloaded: --accept-amd-license, as NVIDIA's for DLSS"""
    def test_accepted(self):
        self.assertTrue(fsr.accepted(True, interactive=False))
        self.assertFalse(fsr.accepted(False, interactive=False))   # (no one to ask: no)
        self.assertTrue(fsr.accepted(False, ask=lambda _: " Yes ", interactive=True))
        self.assertFalse(fsr.accepted(False, ask=lambda _: "no", interactive=True))

        def eof(_):
            raise EOFError
        self.assertFalse(fsr.accepted(False, ask=eof, interactive=True))

    def test_script_downloads_nothing_unaccepted(self):
        with mock.patch.object(fsr, "fetch", side_effect=AssertionError("downloaded without the license")), \
                mock.patch.object(fsr, "accepted", return_value=False), \
                mock.patch("sys.stdout", io.StringIO()), mock.patch("sys.stderr", io.StringIO()) as err:
            self.assertEqual(fetch.main(["fsr", "--game", "unused"]), 2)
        self.assertIn("--accept-amd-license", err.getvalue())


class BuilderTests(unittest.TestCase):
    """longshot-builder downloads the same files at the same commit for a player's build (runtimes.rs, which also refuses
    to download before the license is accepted, --accept-licenses, and leaves a player's own DLL alone: its own tests)"""
    def test_the_builder_pins_the_same_files(self):
        sys.path.insert(0, str(ROOT / "tests"))
        from vendor_pins import builder_pins
        repo, commit, files = builder_pins("FSR")
        self.assertEqual(commit, fsr.commit)
        self.assertIn(repo, fsr.base)
        self.assertEqual([tuple(f) for f in files], [tuple(f) for f in fsr.files])


if __name__ == "__main__":
    unittest.main()
