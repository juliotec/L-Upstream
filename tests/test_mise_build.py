"""The mise tasks: the front door is a handful of them, and the build and development ones forward their arguments to the
scripts unchanged; `mise run build` adds a work folder of the development folder's (`dev.py paths work`) when none is given,
never the player's builder folder."""

import json
import os
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def tasks():
    with (ROOT / "mise.toml").open("rb") as f:
        return tomllib.load(f)["tasks"]


class MiseBuildTest(unittest.TestCase):

    def test_task_declaration(self):
        t = tasks()
        build = t["build"]
        self.assertEqual(build["shell"], "bash -c")
        self.assertEqual(build["run"], "python3 -I scripts/dev.py packs run --")
        self.assertIn("longshot-builder", build["description"])
        self.assertIn("packs run -- compile", t["build-dev"]["run"])
        self.assertIn("never measured", t["build-dev"]["description"])
        self.assertEqual(t["dev"]["run"], 'python3 -I scripts/dev.py "$@"')
        for name in ("build", "build-dev", "dev", "setup"):
            self.assertNotIn("build-deps", t[name].get("depends", []), name)   # (no Python packages for a build)
        self.assertNotIn("build-deps", t)
        self.assertEqual(t["setup"]["run"], "python3 -I scripts/setup.py")
        self.assertIn("dev tools install . --system", t["tools"]["run"])
        for check in ("cargo fmt --all --check", "cargo clippy --workspace --all-targets --locked -- -D warnings",
                      "cargo test --workspace --locked"):
            self.assertIn(check, t["rust"]["run"])
        # (a non-Linux setup goes on without the checks' Linux tools)
        self.assertIn("exit 0", t["test-deps"]["run"].split("fi", 1)[0])
        shown = sorted(n for n, task in t.items() if not task.get("hide"))
        # (tools: setup's step that puts tools.lock's build tools in the checkout, as a player's build has them; setup.md)
        self.assertEqual(shown, ["build", "build-dev", "ci", "dev", "lint", "rust", "setup", "test", "test-changed",
                                 "tools", "tracy-tools"],
                         "a new task is one more thing to learn: fold it into one of these, or hide a helper")

    def test_rust_is_pinned(self):
        """mise.toml's Rust is the one docs/dev/reference/builder.md and scripts/renhdrum.py (the release's pinned Rust) name"""
        with (ROOT / "mise.toml").open("rb") as f:
            rust = tomllib.load(f)["tools"]["rust"]
        renhdrum = (ROOT / "scripts" / "renhdrum.py").read_text(encoding="utf-8")
        self.assertIn('RUST_VERSION = "%s"' % rust["version"], renhdrum)
        builder = (ROOT / "docs" / "dev" / "reference" / "builder.md").read_text(encoding="utf-8")
        self.assertIn("Rust %s, as `mise.toml` pins it (`scripts/renhdrum.py`'s `RUST_VERSION` pins the same" % rust["version"],
                      builder)
        self.assertIn("clippy", rust["components"])

    def test_windows_tooling_and_graceful_launcher(self):
        """pre-commit comes from PyPI (Aqua has no Windows build), and scripts/run-vk.ps1 -Graceful closes the game's window
        before it stops it."""
        with (ROOT / "mise.toml").open("rb") as f:
            config = tomllib.load(f)
        self.assertEqual(config["tools"]["pypi:pre-commit"], "4.0.1")
        self.assertNotIn("aqua:pre-commit/pre-commit", config["tools"])
        self.assertNotIn("aqua:pre-commit/pre-commit", (ROOT / "mise.lock").read_text(encoding="utf-8"))

        launcher = (ROOT / "scripts" / "run-vk.ps1").read_text(encoding="utf-8")
        self.assertIn("[switch]$Graceful", launcher)
        self.assertIn("-Graceful requires -Window", launcher)
        self.assertIn("if ($Graceful)", launcher)
        self.assertIn("$p.CloseMainWindow()", launcher)
        self.assertIn("$p.WaitForExit(10000)", launcher)
        self.assertLess(launcher.index("$p.CloseMainWindow()"), launcher.index("Stop-Process -Id $p.Id"))
        self.assertIn("if ($Graceful -and -not $exited) { exit 1 }", launcher)
        self.assertIn("if ($Graceful) { exit $p.ExitCode }", launcher)

    @unittest.skipUnless(shutil.which("mise") and shutil.which("bash") and shutil.which("python3"),
                         "the mise build task needs mise, bash and python3")
    def test_arguments_arrive_unchanged(self):
        """`mise run build` with a fake dev.py, with no dump or real build, covering spaces and a trailing backslash: the
        arguments arrive at `dev.py packs run --` as given (which adds the development folder's work/ when they name none:
        test_dev_packs)"""
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / "scripts").mkdir()
            # (TOML's basic strings take JSON's escapes)
            (root / "mise.toml").write_text(
                '[tasks.build]\ndescription = "argv recorder"\nshell = "bash -c"\nrun = %s\n' % json.dumps(tasks()["build"]["run"]),
                encoding="utf-8")
            (root / "scripts" / "dev.py").write_text(
                "import json, sys\nassert sys.argv[1:4] == ['packs', 'run', '--'], sys.argv\nprint(json.dumps(sys.argv[4:]))\n",
                encoding="utf-8")
            args = ["build", "--game", "Game [v131072] with spaces.nup", "--out", "C:\\Fake Folder\\", "--language", "en-US"]
            for given, expected in ((args, args), (args + ["--work", "my work"], args + ["--work", "my work"])):
                with self.subTest(given=given):
                    done = subprocess.run(["mise", "run", "--yes", "build", *given], cwd=root, env=os.environ.copy(),
                                          text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60)
                    output = done.stdout.strip().splitlines()
                    self.assertEqual(done.returncode, 0, done.stdout)
                    self.assertTrue(output, "mise produced no argv to check")
                    self.assertEqual(json.loads(output[-1]), expected)

if __name__ == "__main__":
    unittest.main()
