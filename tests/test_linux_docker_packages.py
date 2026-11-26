"""builder/tools/Dockerfile is the Linux build environment for any distribution: what the build tools are made in
(builder/tools/make.sh, their sysroot among them) and what CI builds the Linux longshot-builder in
(.forgejo/workflows/build-linux.yml). Reads both files; nothing is built.

The two are kept by hand. The base system is the same, or a newer one puts a newer glibc in the sysroot and the game a
player builds no longer runs on the systems the release is for. Every package the workflow installs is in the image, or
CI's build fails in the container. The image has more (the recipes' own builds), and the toolchain PPA's GCC 13: the
sysroot's recipe copies the newest GCC's runtime into it, and the game needs libstdc++ 13 (C++20 <format>).

    python -I -B tests/test_linux_docker_packages.py
"""
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCKERFILE = ROOT / "builder/tools/Dockerfile"
WORKFLOW = ROOT / ".forgejo/workflows/build-linux.yml"
TOOLS_WORKFLOW = ROOT / ".forgejo/workflows/tools.yml"


def installs(text):
    """Every package an apt-get install names, with the commands' line continuations joined."""
    text = re.sub(r"\\\n", " ", text)
    packages = set()
    for args in re.findall(r"apt-get install([^&;\n]*)", text):
        packages.update(a for a in args.split() if not a.startswith("-"))
    return packages


def ppas(text):
    """Repository operands, not add-apt-repository's -y/--no-update flags."""
    text = re.sub(r"\\\n", " ", text)
    return set(re.findall(r"add-apt-repository\s+(?:-\S+\s+)*(\S+)", text))


class LinuxDockerPackages(unittest.TestCase):
    def setUp(self):
        self.docker = DOCKERFILE.read_text(encoding="utf-8")
        self.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_base_image(self):
        ci = re.search(r"^\s*container:\s*(\S+)", self.workflow, re.M)
        base = re.search(r"^FROM\s+(\S+)", self.docker, re.M)
        self.assertIsNotNone(ci, "the workflow's container")
        self.assertIsNotNone(base, "the Dockerfile's FROM")
        self.assertEqual(base.group(1), ci.group(1))

    def test_packages(self):
        ci, docker = installs(self.workflow), installs(self.docker)
        self.assertIn("build-essential", ci, "the workflow's package list was not found")
        self.assertEqual(sorted(ci - docker), [], "in the workflow, not in the Dockerfile")
        self.assertLessEqual({"g++-13", "libstdc++-13-dev"}, docker, "the Dockerfile has no GCC 13 for the sysroot")

    def test_the_make_tools_workflow_is_the_image(self):
        """The Make tools workflow makes the archives a developer makes in the image (builder/tools/docker.sh): the same
        system, packages and PPA"""
        wf = TOOLS_WORKFLOW.read_text(encoding="utf-8")
        self.assertEqual(re.search(r"^\s*container:\s*(\S+)", wf, re.M).group(1), "ubuntu:22.04")
        self.assertEqual(sorted(installs(wf) ^ installs(self.docker)), [])
        self.assertEqual(ppas(wf), ppas(self.docker))

    def test_ppa_options(self):
        ppa = "ppa:example/testing"
        for flags in ("", "-y ", "--no-update ", "-y --no-update ", "--no-update -y "):
            with self.subTest(flags=flags):
                self.assertEqual(ppas("timeout 180s add-apt-repository " + flags + ppa + " && apt-get update"), {ppa})
        self.assertEqual(ppas("add-apt-repository -y \\\n --no-update " + ppa), {ppa})
        self.assertEqual(ppas("apt-get update"), set())

    def test_ppas(self):
        self.assertEqual(ppas(self.docker), {"ppa:ubuntu-toolchain-r/test"}, "the Dockerfile's toolchain PPA (GCC 13)")
        self.assertLessEqual(ppas(self.workflow), ppas(self.docker))

    def test_ppa_from_launchpad_net(self):
        """(main's #2373) where the toolchain PPA is added, its source is rewritten to ppa.launchpad.net:
        ppa.launchpadcontent.net, add-apt-repository's choice, answered 503 for hours and every build that added it failed"""
        rewrite = "sed -i 's|https://ppa\\.launchpadcontent\\.net/|http://ppa.launchpad.net/|g' /etc/apt/sources.list.d/*.list"
        files = [DOCKERFILE, WORKFLOW] + sorted((ROOT / ".forgejo/workflows").glob("*.yml"))
        adders = 0
        for f in dict.fromkeys(files):
            text = re.sub(r"\\\n", " ", f.read_text(encoding="utf-8")) if f.exists() else ""
            for m in re.finditer(r"add-apt-repository\s+(?:-\S+\s+)*ppa:ubuntu-toolchain-r/test", text):
                adders += 1
                with self.subTest(file=f.relative_to(ROOT).as_posix()):
                    after = text[m.end():]
                    self.assertIn(rewrite, after, "the PPA's source is not rewritten")
                    self.assertLess(after.index(rewrite), after.index("apt-get update"), "rewritten after the update")
        self.assertGreaterEqual(adders, 2, "the Dockerfile's and the Make tools workflow's PPA were not found")


if __name__ == "__main__":
    unittest.main()
