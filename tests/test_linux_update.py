"""The Linux build's sysroot (builder/tools/linux-x64/sysroot.sh): its system libraries' aliases carried as their bytes, its one
relative link kept (packed as tools.lock's archive, `longshot-builder dev tools make`, which keeps a relative link: build-
driver's tools/archive.rs test pack_is_deterministic_with_modes_and_links). (The game folder's launcher and bin/
went with the old builder: longshot-builder puts the game in the folder itself; the sysroot's place is named by what it is
in the object keys, builder/crates/build-compile/src/key.rs.)

python -I -B tests/test_linux_update.py
"""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

# (Linux only)
RUN_ALL_SKIP = {"windows": "the Linux sysroot, POSIX files and shell scripts"}

ROOT = Path(__file__).resolve().parents[1]


class SysrootTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform.startswith("linux"), "the Linux sysroot copy loop")
    def test_sysroot_system_aliases_round_trip_as_library_bytes(self):
        # (the sysroot is made by its recipe, then packed as tools.lock's archive, which the builder carries)
        script = (ROOT / "builder" / "tools" / "linux-x64" / "sysroot.sh").read_text(encoding="utf-8")
        start = script.index("for f in crt1.o ")
        loop = script[start:script.index("\ndone", start) + len("\ndone")]
        with tempfile.TemporaryDirectory(prefix="longshot-sysroot-") as tmp:
            root = Path(tmp)
            system = root / "system"
            system.mkdir()
            real = system / "libmvec.so.1"
            real.write_bytes(b"private library bytes")
            alias = system / "libmvec.so"
            alias.symlink_to(real)
            source = root / "sysroot"
            (source / "usr" / "lib" / "x86_64-linux-gnu").mkdir(parents=True)
            (source / "lib").symlink_to("usr/lib")
            env = dict(os.environ, M=str(system), S=str(source))
            subprocess.run(["bash", "-euo", "pipefail", "-c", loop], env=env, check=True)
            copied = source / "usr" / "lib" / "x86_64-linux-gnu" / alias.name
            self.assertFalse(copied.is_symlink(), "the sysroot must carry the alias's bytes, not an absolute target")
            self.assertEqual(os.readlink(source / "lib"), "usr/lib")   # (the one link: relative, as it is packed)
            self.assertEqual(copied.read_bytes(), real.read_bytes())
            self.assertEqual(os.readlink(alias), str(real))
            self.assertEqual(real.read_bytes(), b"private library bytes")


if __name__ == "__main__":
    unittest.main()
