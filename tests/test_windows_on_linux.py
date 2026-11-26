"""Game-free checks that Longshot's Windows files are made on the Linux runner (docs/dev/reference/ci.md: there is no Windows
one): no workflow asks for a Windows runner, which would queue forever and hold a release; the Linux llvm-mingw that compiles
them is tools.lock's release; the Windows longshot-builder's job checks what it made and the bundle's dav1d (no dav1d is
built in CI); and the Vulkan back end's build check, which ships nothing, is ci.yml's (Linux and Windows, without and
with AMD FSR 4, when a change touches the renderer), not a release's.

Run with: python -I tests/test_windows_on_linux.py
"""
import importlib.util
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = ROOT / ".forgejo" / "workflows"


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


class WindowsOnLinuxTests(unittest.TestCase):
    def test_no_windows_runner(self):
        for wf in sorted(WORKFLOWS.glob("*.yml")):
            text = wf.read_text(encoding="utf-8")
            with self.subTest(wf.name):
                self.assertIsNone(re.search(r"runs-on:.*windows", text, re.I))
                # (a matrix's os, which runs-on names through ${{ matrix.os }})
                self.assertIsNone(re.search(r"^\s*-?\s*os:\s*windows", text, re.I | re.M))

    def test_linux_llvm_mingw_is_tools_lock_release(self):
        package = module("longshot_build_under_test", ROOT / "scripts" / "longshot_builder.py")
        lock = (ROOT / "tools.lock").read_text(encoding="utf-8").splitlines()
        windows = [l.split()[2] for l in lock if l.startswith("tool llvm-mingw ") and l.split()[3] == "windows-x64"]
        version, prefix, sha256, url = package.linux_llvm_mingw()
        self.assertEqual(len(windows), 1)
        self.assertTrue(windows[0].startswith(version + "-"), (windows, version))
        self.assertEqual(prefix, "llvm-mingw-%s-ucrt-ubuntu-22.04-x86_64" % version)
        self.assertRegex(sha256, r"^[0-9a-f]{64}$")
        self.assertEqual(url, "https://github.com/mstorsjo/llvm-mingw/releases/download/%s/%s.tar.xz" % (version, prefix))

    def test_the_windows_job(self):
        wf = (WORKFLOWS / "build-windows.yml").read_text(encoding="utf-8")
        self.assertIn("scripts/longshot_builder.py build --target windows --profile dist", wf)
        self.assertIn('scripts/longshot_builder.py check "dist/longshot-builder-$TAG-windows-x64.exe"', wf)
        self.assertIn('"dist/longshot-builder-$TAG-windows-x64.exe" sources', wf)   # (started under Wine)
        for name in ("build-windows.yml", "vulkan-dlss.yml"):
            text = (WORKFLOWS / name).read_text(encoding="utf-8")
            with self.subTest(name):
                self.assertNotIn("NUMBER_OF_PROCESSORS", text)   # (Windows' own variable)
                # (each command that runs it, not a comment naming it)
                for line in re.findall(r"^(?![ \t]*#).*longshot_builder\.py llvm-mingw.*", text, re.M):
                    self.assertIn("--host linux", line)

    def test_dav1d_is_the_bundles(self):
        # (dav1d comes from tools.lock's cpm package, in the bundle: CI checks it under Wine and builds none of its own;
        # builder/tools/windows-x64/dav1d.sh makes it, part of the package's recipe)
        wf = (WORKFLOWS / "build-windows.yml").read_text(encoding="utf-8")
        self.assertNotIn("dav1d.sh", wf)
        self.assertNotIn("meson", wf)
        package = wf[wf.index("\n  package:"):]   # (its only job)
        self.assertIn('dev tools install "$PWD" --system windows-x64', package)
        self.assertRegex(package, r"tests/test_dav1d_av1\.py --cross \"\$LLVM_MINGW/bin/x86_64-w64-mingw32-clang\" \\\n\s+--run ")
        self.assertLess(package.index("Wine, installed"), package.index("test_dav1d_av1.py"))
        self.assertTrue((ROOT / "builder" / "tools" / "windows-x64" / "dav1d.sh").is_file())

    def test_the_back_end_is_checked_in_ci_not_in_a_release(self):
        # (the Vulkan back end alone, which ships nothing, is built by ci.yml's renderer job on the pull request runners,
        # for Linux and for Windows, without and with AMD FSR 4; a release builds only what it ships, so nothing of a
        # release's skips it)
        release = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        self.assertNotIn("released-as", release)
        self.assertNotIn("outputs.checked", release)
        for name in ("build-linux.yml", "build-windows.yml"):
            text = (WORKFLOWS / name).read_text(encoding="utf-8")
            with self.subTest(name):
                self.assertNotIn("compile --only renderer", text)
                self.assertNotIn("RELEASE_CHECKED", text)
                self.assertNotIn("inputs.checked", text)
        ci = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
        job = ci[ci.index("\n  renderer:\n"):]
        job = job[:job.index("\n\n  # ")]
        self.assertIn("if: needs.changes.outputs.renderer == 'true'", job)
        self.assertIn("runs-on: ubuntu-24.04", job)
        self.assertNotRegex(job, r"runs-on: *(release|deploy)")
        self.assertIn("system: linux-x64", job)
        self.assertIn("system: windows-x64", job)
        self.assertEqual(job.count("\"$BUILDER\" compile --only renderer"), 2)
        self.assertEqual(job.count("--upscaler fsr4"), 1)
        self.assertIn('grep -q "with AMD FSR 4"', job)
        # (a base whose ci_changes.py says no renderer= falls back to code=, then to everything)
        self.assertIn("renderer: ${{ steps.files.outputs.renderer || steps.files.outputs.code || 'true' }}", ci)
        changes = module("ci_changes_under_test", ROOT / "scripts" / "ci_changes.py")
        for f in ("gpu/vk/vk_draw.cpp", "host/hos_tracy.h", "host/posix/win32_posix.c", "tools.lock", "builder/Cargo.toml",
                  "third_party/imgui/imgui.cpp", ".forgejo/workflows/ci.yml"):
            self.assertTrue(changes.needs_renderer([f]), f)
        for f in ("docs/dev/reference/ci.md", "host/hos_main.c", "game/phases.c", "website/index.html",
                  ".forgejo/workflows/release.yml"):
            self.assertFalse(changes.needs_renderer([f]), f)
        self.assertTrue(changes.needs_renderer([]))   # (no file: unsure, it runs)


if __name__ == "__main__":
    unittest.main()
