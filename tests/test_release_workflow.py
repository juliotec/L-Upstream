"""Game-free checks of the release's workflows (.forgejo/workflows/release.yml, build-linux.yml, build-windows.yml;
docs/dev/reference/releases.md) and of the Make tools workflow (tools.yml): the jobs on the deploy runner, which opens the
sealed secrets, compile no Rust and run no program the checkout builds, and give each secret to the steps that use it
only; the jobs that make the release's files run only on the server's release runner, restore only their own caches (downloads, and the builder's compiled Rust for trusted runs) and build
nothing they don't ship (the back end's build check is ci.yml's); each build checks tools.lock before it uploads anything; a release uploads no tool and stops on a line whose archive is not in the store;
tools.yml uploads only on main, what its make job made twice; and scripts/ci_tools_publish.py refuses an archive whose
version tools.lock names with other bytes before it uploads anything. Reads the files, runs nothing over the network.

Run with: python -I tests/test_release_workflow.py
"""
import contextlib
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import re
import shutil
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = ROOT / ".forgejo" / "workflows"


def text(name):
    return (WORKFLOWS / name).read_text(encoding="utf-8")


def jobs(workflow):
    """{job id: its text} of a workflow's jobs: block"""
    body = workflow[workflow.index("\njobs:\n"):]
    starts = [(m.start(), m.group(1)) for m in re.finditer(r"^  ([a-z][a-z0-9-]*):\n", body, re.M)]
    return {name: body[at:(starts[i + 1][0] if i + 1 < len(starts) else len(body))] for i, (at, name) in enumerate(starts)}


def job_env(job):
    """The job's own env: block (not a step's)"""
    m = re.search(r"^    env:\n((?:      .*\n)+)", job, re.M)
    return m.group(1) if m else ""


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


class ReleaseWorkflowTests(unittest.TestCase):
    def test_the_deploy_runner_builds_nothing(self):
        for workflow, name in (("release.yml", "publish"), ("tools.yml", "publish")):
            job = jobs(text(workflow))[name]
            with self.subTest(workflow=workflow, job=name):
                self.assertIn("runs-on: deploy", job)
                self.assertNotIn("cargo", job)
                self.assertNotIn("ci_rust.sh", job)
                self.assertNotIn("target/debug", job)
                self.assertNotIn("longshot-builder dev", job)
                self.assertNotIn("make.sh", job)
                # (the secrets in the steps that open or use them, not the job's env, which every step sees)
                env = job_env(job)
                self.assertNotIn("secrets.", env)
                self.assertNotIn("forgejo.token", env)
                self.assertNotIn("RELEASE_TOKEN", env)
        self.assertIn("python3 -I scripts/ci_tools_publish.py made --lock tools.lock", jobs(text("tools.yml"))["publish"])

    def test_a_release_uploads_no_tool(self):
        release = text("release.yml")
        self.assertEqual(sorted(jobs(release)), ["linux", "publish", "version", "windows"])
        for gone in ("PACKAGES_TOKEN", "ci_tools_publish", "make.sh", "dev tools missing", "dev tools publish"):
            self.assertNotIn(gone, release)
        # (each build checks tools.lock with its runner's builder, before it uploads anything; publish needs both)
        for name in ("build-linux.yml", "build-windows.yml"):
            with self.subTest(name):
                wf = text(name)
                tools = wf[wf.index("- name: The tools"):]
                self.assertIn("$TOOLS check --require", tools)
                self.assertIn("Run the Make tools workflow", tools)
                self.assertLess(wf.index("- name: The tools"), wf.index("uses: https://data.forgejo.org/forgejo/upload-artifact@"))
        self.assertIn("needs: [version, linux, windows]", jobs(release)["publish"])

    def test_the_make_tools_workflow(self):
        wf = text("tools.yml")
        self.assertRegex(wf, r"(?m)^on:\n  workflow_dispatch:\n")
        self.assertNotRegex(wf, r"(?m)^  (push|pull_request|schedule):")
        make, publish = jobs(wf)["make"], jobs(wf)["publish"]
        self.assertIn("builder/tools/make.sh --out made --check", make)
        self.assertNotIn("secrets.", make)
        # (the input reaches the script through env, never pasted into it)
        self.assertNotRegex(make, r"run:[^\n]*\$\{\{ *inputs")
        self.assertIn("WANTED: ${{ inputs.tools }}", make)
        self.assertIn("if: github.ref == 'refs/heads/main'", publish)
        self.assertIn("needs: make", publish)
        self.assertIn("secrets.PACKAGES_TOKEN_SEALED", publish)
        self.assertIn("enable-openid-connect: true", publish)

    def test_the_builds_restore_only_their_downloads_and_check_nothing_they_dont_ship(self):
        for name, artifact in (("build-linux.yml", "longshot-builder-linux"), ("build-windows.yml", "longshot-builder-windows")):
            workflow = text(name)
            with self.subTest(name):
                # (the server's release runner only, checked before anything is built)
                guard = workflow.index("run: scripts/ci_release_runner.sh")
                self.assertLess(guard, workflow.index("longshot_builder.py build"))
                # (the release's own caches, main's and the tags' runs only: the downloads, each checked by its SHA-256
                # when used, never an unpacked Rust; and the shipped builder's compiled Rust alone (cargo's target folder,
                # trusted runs only: ci.md, The release's cache), never a key the checks write)
                kinds = set()
                for m in re.finditer(r"uses: https://github.com/actions/cache/(?:restore|save)@", workflow):
                    step = workflow[workflow.rindex("\n      - ", 0, m.start()):m.end() + 600].split("\n      - ")[1]
                    self.assertIn("github.ref == 'refs/heads/main' || startsWith(github.ref, 'refs/tags/v')", step)
                    if "key: release-rust-target-" in step:
                        kinds.add("rust-target")
                        self.assertIn("path: ${{ runner.temp }}/rust-work/target\n", step)
                        self.assertRegex(step, r"key: release-rust-target-(linux|windows)-dist-[a-z0-9_-]+-\$\{\{ hashFiles\('scripts/renhdrum\.py'\) \}\}-\$\{\{ hashFiles\('Cargo\.lock'\) \}\}\n")
                        continue
                    self.assertRegex(step, r"key: release-downloads-")
                    kinds.add("downloads")
                    # (one path a line: a path starts with ${{ runner.temp }}, which has spaces)
                    paths = [p.strip() for p in re.search(r"path: \|\n((?:            .*\n)+)", step).group(1).splitlines()]
                    for path in paths:
                        self.assertRegex(path, r"/(rust/\*\.tar\.xz|rust/crates|tools-cache|llvm/\.cache|link-tools/\.cache)$")
                self.assertEqual(kinds, {"downloads", "rust-target"})
                # (the compiled Rust restored right before the shipped builder's one compile, and saved right after it)
                build = workflow.index("longshot_builder.py build --target")
                self.assertLess(workflow.index("id: rust-target"), build)
                self.assertLess(build, workflow.index("key: release-rust-target-", workflow.index("id: rust-target") + 300))
                upload = workflow.index("name: " + artifact)
                for made in ("dev release files", "dev release recomplauncher"):
                    self.assertLess(workflow.index(made), upload)
                # (no build check that ships nothing: the back end's is ci.yml's renderer job)
                self.assertNotIn("compile --only renderer", workflow)
        self.assertEqual(sorted(jobs(text("build-windows.yml"))), ["package"])

    def test_the_builder_is_compiled_once_per_platform(self):
        # (each job compiles the program it ships once, the dist profile, and attaches its pack to it after the link: no
        # second builder for the runner, no pack compiled in. Windows runs after Linux and takes the Linux program, before
        # its pack, as its tool: an artifact of the same run, fetched after the release runner's guard)
        for name, target in (("build-linux.yml", "linux"), ("build-windows.yml", "windows")):
            wf = text(name)
            with self.subTest(name):
                builds = re.findall(r"^(?![ \t]*#).*longshot_builder\.py build .*", wf, re.M)
                self.assertEqual(len(builds), 1, builds)
                self.assertIn("--target %s --profile dist" % target, builds[0])
                for gone in ("--this-pc", "--sources", "--profile release", "LONGSHOT_SOURCES"):
                    self.assertNotIn(gone, wf)
                self.assertIn("dev release attach", wf)
                attach, check = wf.index("dev release attach"), wf.index("scripts/longshot_builder.py check")
                self.assertLess(attach, check)
        # (the shipped Linux program linked as the game is: tools.lock's clang and sysroot, fetched before the compile)
        linux = text("build-linux.yml")
        self.assertIn('--linker "$RUNNER_TEMP/link-tools/llvm/bin/clang" --sysroot "$RUNNER_TEMP/link-tools/sysroot"', linux)
        self.assertLess(linux.index("longshot_builder.py link-tools"), linux.index("longshot_builder.py build --target"))
        linux, windows = text("build-linux.yml"), text("build-windows.yml")
        self.assertIn("name: longshot-builder-linux-tool\n          path: ${{ runner.temp }}/host/longshot-builder", linux)
        tool = windows.index("name: longshot-builder-linux-tool")
        self.assertLess(windows.index("run: scripts/ci_release_runner.sh"), tool)
        self.assertLess(tool, windows.index("- name: The tools"))
        self.assertIn("needs: [version, linux]", jobs(text("release.yml"))["windows"])
        # (publish takes the release's files only, never the tool)
        self.assertNotIn("longshot-builder-linux-tool", text("release.yml"))

    def test_the_third_party_objects_seed_from_the_last_signed_release(self):
        # (dev release last-objects checks the release's signed SHA256SUMS first; the set it writes reaches the one
        # compile that makes the release's set, never a later step)
        for name, platform in (("build-linux.yml", "linux-x64"), ("build-windows.yml", "windows-x64")):
            wf = text(name)
            with self.subTest(name):
                step = wf[wf.index("- name: The third-party objects"):]
                step = step[:step.index("\n      - ", 1)]
                self.assertIn('dev release last-objects --version "$TAG" --platform %s' % platform, step)
                self.assertLess(step.index("last-objects"), step.index("--third-party-objects"))
                self.assertIn('LONGSHOT_THIRD_PARTY_OBJECTS="$last"', step)
                self.assertNotIn("GITHUB_ENV", step)
                self.assertEqual(wf.count("LONGSHOT_THIRD_PARTY_OBJECTS"), 1)

    def test_the_pack_reuses_the_last_signed_release_s_members_and_uploads_are_stored(self):
        # (the pack's members that did not change come from the pack last-objects kept, the one its signed SHA256SUMS
        # named; the downloads are saved once the last is made, before the steps that can still fail; what dist/ holds is
        # compressed already, so it is zipped stored)
        for name in ("build-linux.yml", "build-windows.yml"):
            wf = text(name)
            with self.subTest(name):
                sources = wf.index("dev release sources --out")
                self.assertLess(wf.index("dev release last-objects"), sources)
                step = wf[sources:wf.index("\n      - ", sources)]
                self.assertIn('--reuse "$RUNNER_TEMP/last-objects/sources.pack"', step)
                self.assertLess(wf.index("- name: Save the release's downloads"), sources)
                self.assertLess(wf.index("longshot_builder.py build --target"), wf.index("- name: Save the release's downloads"))
                self.assertLess(wf.index("scripts/renhdrum.py build"), wf.index("- name: Save the release's downloads"))
                uploads = re.findall(r"uses: https://data\.forgejo\.org/forgejo/upload-artifact@.*\n        with:\n((?:          .*\n)+)", wf)
                self.assertTrue(uploads)
                for w in uploads:
                    level = "1" if "/host/longshot-builder\n" in w else "0"
                    self.assertIn("compression-level: %s\n" % level, w)

    def test_the_version_builds_nothing(self):
        version = jobs(text("release.yml"))["version"]
        for gone in ("cargo", "ci_rust.sh", "longshot-builder dev", "actions/cache"):
            self.assertNotIn(gone, version)

    def test_node_for_the_actions_is_one_pin(self):
        """The Node a job in ubuntu:22.04 installs for its actions, before the checkout (which needs it), so in each such
        workflow: the same version and SHA-256 in each"""
        pins = {name: sorted(set(re.findall(r"NODE_(?:VER|SHA256): (\S+)", text(name)))) for name in ("build-linux.yml", "tools.yml")}
        self.assertEqual(len(set(map(tuple, pins.values()))), 1, pins)
        self.assertEqual(len(pins["tools.yml"]), 2)


class ToolsPublishTests(unittest.TestCase):
    def setUp(self):
        self.p = module("ci_tools_publish_under_test", ROOT / "scripts" / "ci_tools_publish.py")
        self.dir = tempfile.mkdtemp(prefix="ci-tools-publish-")
        self.addCleanup(shutil.rmtree, self.dir, True)

    def lock(self, *tools):
        """A lock of (name, version, system, sha, size) lines"""
        lock = os.path.join(self.dir, "tools.lock")
        with open(lock, "w", encoding="utf-8") as f:
            f.write("# a comment\n")
            for name, version, system, sha, size in tools:
                f.write(self.p.line({"name": name, "version": version, "system": system, "sha256": sha, "size": size}) + "\n")
        return lock

    def made(self, system, name, version, data):
        d = os.path.join(self.dir, "made", system)
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, name + ".tar.zst"), "wb") as f:
            f.write(data)
        with open(os.path.join(d, name + ".version"), "w", encoding="utf-8") as f:
            f.write(version + "\n")
        with open(os.path.join(self.dir, "made", "add.sh"), "w", encoding="utf-8") as f:
            f.write("# make.sh's\n")
        return os.path.join(self.dir, "made")

    def run_it(self, *args, listed=None):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), mock.patch.object(self.p, "put") as put, \
                mock.patch.object(self.p, "listed", side_effect=listed or (lambda t: None)), \
                mock.patch.dict(os.environ, {"RELEASE_TOKEN": "t"}):
            try:
                rc = self.p.main(list(args))
            except SystemExit as e:
                rc = e.code
        return rc, out.getvalue(), put

    def test_a_version_the_lock_names_with_other_bytes_is_refused_before_any_upload(self):
        lock = self.lock(("sdl", "1.0", "windows-x64", "0" * 64, 3))
        rc, out, put = self.run_it(self.made("windows-x64", "sdl", "1.0", b"abc"), "--lock", lock)
        self.assertEqual(rc, 1)
        self.assertIn("A package version's files never change", out)
        put.assert_not_called()

    def test_a_new_version_is_uploaded_checked_and_written_into_the_lock(self):
        data = b"abc"
        sha = hashlib.sha256(data).hexdigest()
        lock = self.lock(("sdl", "1.0", "windows-x64", "0" * 64, 3), ("cpm", "1.0", "windows-x64", "1" * 64, 4))
        made = self.made("windows-x64", "sdl", "2.0", data)
        new = os.path.join(self.dir, "new.lock")
        rc, out, put = self.run_it(made, "--lock", lock, "--write-lock", new, listed=lambda t: (t["size"], t["sha256"]))
        self.assertEqual(rc, 0, out)
        put.assert_called_once()
        want = self.p.line({"name": "sdl", "version": "2.0", "system": "windows-x64", "sha256": sha, "size": 3})
        self.assertIn(want, out)
        with open(new, encoding="utf-8") as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[0], "# a comment")
        self.assertEqual(lines[1], want, "in place of the tool's old line")
        self.assertTrue(lines[2].startswith("tool cpm 1.0 windows-x64 "), "the other lines kept")

    def test_an_upload_not_in_the_store_after_fails(self):
        lock = self.lock()
        rc, out, put = self.run_it(self.made("linux-x64", "sdl2", "2.0", b"abc"), "--lock", lock)
        put.assert_called_once()
        self.assertEqual(rc, 1, "listed() answers none: the copy is not in the store")
        self.assertIn("its copy is not in the store", out)

    def test_an_archive_without_its_version_is_refused(self):
        made = self.made("linux-x64", "sdl2", "2.0", b"abc")
        os.remove(os.path.join(made, "linux-x64", "sdl2.version"))
        rc, out, put = self.run_it(made, "--lock", self.lock())
        self.assertEqual(rc, 1)
        self.assertIn("no sdl2.version", out)
        put.assert_not_called()

    def test_the_upload_goes_to_the_upload_api(self):
        with mock.patch.dict(os.environ, {"FORGEJO_UPLOAD_API": "http://10.0.0.2:3000/api/v1"}):
            self.assertEqual(self.p.upload_server(), "http://10.0.0.2:3000")
        with mock.patch.dict(os.environ, {"FORGEJO_UPLOAD_API": ""}):
            self.assertEqual(self.p.upload_server(), "https://git.magicbean.dev")

    def test_the_store_path_is_lock_rs(self):
        # (build-driver's tools/lock.rs Tool::store_url: the same address)
        rs = (ROOT / "builder" / "crates" / "build-driver" / "src" / "tools" / "lock.rs").read_text(encoding="utf-8")
        self.assertIn('"{server}/api/packages/{OWNER}/generic/{}-{}/{}/{}-{}{EXT}"', rs)
        self.assertIn('pub const OWNER: &str = "MagicBeanDev";', rs)
        t = {"name": "llvm", "version": "23.1.5-x64-1", "system": "linux-x64", "sha256": "a" * 64}
        self.assertEqual(self.p.store_path(t),
                         "/api/packages/MagicBeanDev/generic/llvm-linux-x64/23.1.5-x64-1/%s-llvm.tar.zst" % ("a" * 64))

    def test_the_real_lock_s_lines_are_the_ones_it_writes(self):
        lines = [l for l in (ROOT / "tools.lock").read_text(encoding="utf-8").splitlines() if l.startswith("tool ")]
        tools = self.p.read_lock(str(ROOT / "tools.lock"))
        self.assertEqual(len(tools), len(lines))
        for t, l in zip(tools, lines):
            self.assertEqual(self.p.line(t), l)


if __name__ == "__main__":
    unittest.main()
