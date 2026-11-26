# Releases

How Longshot is versioned, released and delivered to players. A release is made by pushing a tag: CI checks the
number, builds `longshot-builder` for each platform, signs and publishes. There are no release pull requests, no release branches and no
changelog file.

The Longshot Launcher is released the same way from its own repository,
[MagicBeanDev/LongshotLauncher](https://git.magicbean.dev/MagicBeanDev/LongshotLauncher), with its own numbers (from
`v1.0.0`) and the same `release.py`. This page covers Longshot.

`release.py` is Tooling's ([scripts/release.py](https://git.magicbean.dev/MagicBeanDev/Tooling/src/branch/main/scripts/release.py)):
run it from this repository's root as `python -I ../Tooling/scripts/release.py`. CI clones Tooling at the commit
[`.tooling-rev`](../../../.tooling-rev) names, `main`'s for a tag's checks: a change to the release is a Tooling pull
request, then one here that moves `.tooling-rev` to its commit.

## What a release is

One Forgejo release of MagicBeanDev/Longshot: a tag, a page with the notes, and these files.

| File | What it is |
|---|---|
| `longshot-builder-<v>-linux-x64`, `longshot-builder-<v>-windows-x64.exe` | the builder, one self-contained program per platform (about 162 MB and 99 MB): the sources it builds the game from (`host/`, `gpu/`, the `third_party/` and assets the build reads, `VERSION`), the prebuilt third-party objects and the build tools, in one pack appended to the program after it is linked ([what it carries](builder.md#what-a-release-carries)) |
| `longshot-builder-<from>-to-<v>-<platform>.zst-patch` | a delta to it from each of the channel's last 5 releases, and a nightly's from the latest stable with the platform's file too (a player on a stable who switches to nightlies): `zstd --patch-from=<that release's file> --long=30`, about 760 KB for a one-file change |
| `longshot-builder-<v>-<platform>.json` | the manifest: the full file's name, size and SHA-256, and each delta's, with the file it applies to |
| `longshot-builder-<v>-<platform>-recomplauncher.zip` | for [RecompLauncher](#recomplauncher): the same program, as `Longshot.exe` (made a windowed program) or `Longshot.x86_64`, and `recomplauncher.json`, stored |
| `SHA256SUMS`, `SHA256SUMS.sig` | each file's SHA-256 (`sha256sum` format), and its signature by the release key |

A release carries no game and no Vulkan back end: the player's own build compiles the back end from the sources for
their PC. A release's files never change: a fix is the next version. Never put game files, keys, builds or anything
made from a dump in a release ([CONTRIBUTING.md](../../../CONTRIBUTING.md#the-rule-with-no-exceptions)).

Players build through the Longshot Launcher only. It follows a channel and installs a new `longshot-builder` by asking
the builder it has: [`longshot-builder fetch`](build.md#fetch-a-version) checks the signature, then applies a delta from
a version the launcher keeps when the manifest lists one (or a chain of them, below), checking the result against the
full file's SHA-256, or else downloads the full file. The launcher updates itself. Longshot's release pages therefore open with links to the launcher of the same channel and tell players
not to download the files below.

## Versions

Longshot follows [Semantic Versioning](https://semver.org/) from `v3.0.0` (earlier numbers are Hookshot's). One
version covers the game, the Vulkan back end and `longshot-builder`: they are built from one commit.

| Channel | Tag | Made by |
|---|---|---|
| `stable` | `vX.Y.Z` | a maintainer's tag on a tested nightly's commit of `main` |
| `nightly` | `nightly-<count>-<8 hex>` | every push to `main` that brings something for the notes |

A nightly's count is the number of commits since the fork (`e2a4d45`, "Rename the project to Longshot"). A build from a
checkout is named by `git describe --tags --always --dirty` (or `unknown`). It is on no channel, and never offered as
an update.

### The number a tag must have

Pull requests are squash-merged with Conventional Commit titles, so the commits on `main` say what changed.
`release.py check-tag` (CI's first step) refuses a tag lower than they call for:

| Since the last stable, a title with | The next version is at least |
|---|---|
| `!` after the type or scope, or a `BREAKING CHANGE:` footer | the next major: `v4.0.0` after `v3.2.1` |
| `feat` | the next minor: `v3.3.0` |
| `fix`, `perf` or any other type | the next patch: `v3.2.2` |

It also refuses a tag that isn't `vX.Y.Z`, isn't on `main`, or isn't above every release there is. An old release
candidate tag (`vX.Y.Z-rc.N`) is on no channel.

### What the version protects

The public surface is the command line of `longshot-builder`, `longshot-builder --output json`'s events and
error codes ([builder events](build-events.md)), the `RX_` and `HOS_` switches, `longshot_options.ini`,
`longshot_env.txt`, the files a build writes into a game folder (`language.txt`, `longshot_version.txt`,
`longshot_build.json`, `longshot_romfs.idx`), the version folder's layout (`builds/`, `cache/`, `profile/`), the keys of
`profile.json` the builder reads (`game`, `upscalers`), the game's channel to its builder (`LONGSHOT_BUILDER_CHANNEL`), saves, the release files' names, the manifest's format, `SHA256SUMS` and the format of
`longshot_log.txt`. C sources and internal headers are not an API.

### Where the version shows

| Place | Comes from |
|---|---|
| `longshot_log.txt`'s first line and the Insert menu | the compile (`builder/crates/build-compile/src/generated/version.rs`): `LONGSHOT_VERSION`, then a `VERSION` or `version.txt` file, then `git describe --tags --always --dirty --match "v[0-9]*"`, else `unknown` |
| `longshot-builder` and its files | the release workflow's version name (`VERSION` in the pack it carries, and the files' names) |
| `longshot_version.txt` in a game folder | the same release name, written by the build's finish |

The compile also appends a fingerprint of the sources (`host/`, `gpu/`, `builder/crates/`), `(source 1a2b3c4d)`, which
tells two builds of one version apart.

## The release workflow

`.forgejo/workflows/release.yml` runs on every push to `main` and every `v*` tag:

| Job | What it does | Run by |
|---|---|---|
| The version | names the version: a tag is checked; a push gets the nightly name, after waiting for more merges and only when it brings something for the notes. Then, only when there is a version, checks `tools.lock` has every tool the packs carry and every line's archive is in Longshot's store with its size and SHA-256: a release uploads no tool, and stops on a line whose archive isn't there ([bumping a tool](#bumping-a-tool)) | `release.py check-tag`, `nightly-age`, `nightly-news`, `nightly-name`, then `longshot-builder dev tools check`, `dev tools check --require` (a builder built first) |
| longshot-builder (Linux) | compiles the program it ships once (the dist profile, linked by tools.lock's clang against its sysroot, glibc 2.35, the game's floor; `scripts/longshot_builder.py link-tools` fetches both first, each checked by its size and SHA-256), which carries nothing yet and is the job's tool: it installs `tools.lock`'s linux-x64 tools, builds the third-party objects, packs the sources, objects, RenHDrum and tools (`dev release sources`, taking each member that did not change from the last release's pack), and appends the pack to a copy of itself, the release's file (`dev release attach`, read back; its headers then checked); then starts a player's build's dry run from it, makes the stamps, the deltas from the channel's last 5 releases and a nightly's from the latest stable (their full files through `RELEASE_UPLOAD_API` when the runner reaches it; fewer than 5 while more releases have the platform's files is a warning, and so is the latest stable's file that could not be had, though no stable with it yet is not) and the manifest, zips RecompLauncher's file and uploads all of it. It uploads the program before its pack too, as `longshot-builder-linux-tool`, the Windows job's tool. Only the release's own caches are restored ([ci](ci.md#the-releases-cache)) | `build-linux.yml`: `scripts/longshot_builder.py build`, then that builder's `dev tools install`, `dev release last-objects`, `compile --third-party-objects`, `dev release sources`, `dev release attach`, `scripts/longshot_builder.py check`, `dev release dry-run`, `dev release files` (which runs `fetch-bases`, then `deltas`), `dev release recomplauncher` |
| longshot-builder (Windows) | the same for windows-x64, after the Linux job: its tool is the Linux job's program before its pack (an artifact of the run), so it compiles only the program it ships, once, cross-built on Linux with llvm-mingw's Linux build, then attaches its pack and checks it under Wine. Forgejo lists each build's job under the release's, as "longshot-builder (Linux): build" and "longshot-builder (Windows): build" | `build-windows.yml`: `scripts/longshot_builder.py llvm-mingw`, `build`, `check`; the Linux builder's `dev release last-objects`, `sources`, `attach`, `unpack`, `dry-run`, `files`, `recomplauncher` |
| Sign and publish | on the deploy runner, which compiles nothing: checks every platform's files are there (RecompLauncher's zips too), writes and signs `SHA256SUMS`, checks the signature with the key the launcher trusts, writes the notes, uploads everything to a draft and publishes it; a nightly also deletes the nightlies past the newest 20 that are 5 days old or more | `release.py sums`, `sign`, `verify`, `notes`, `publish`, `prune` |

- **Drafts make a release appear at once.** Until the last step no launcher sees it; a failed run leaves a draft, which
  a re-run of the same tag reuses.
- **A release builds only what it ships.** Every file carries its version, so a tag always builds again. The Vulkan
  back end's build checks (Linux and Windows, without and with AMD FSR 4), which ship no file, are `ci.yml`'s
  `renderer` job on the pull requests that touch the renderer and on every push to `main`
  ([ci](ci.md#what-a-pull-request-runs)).
- **Every job runs on Linux.** The Windows files are cross-built; what only Windows can check is a
  [Windows tester's](#windows-tester).
- **The third-party objects come from the last release where they can.** Before compiling them, each build job runs
  `longshot-builder dev release last-objects`: the channel's newest release with the platform's file (up to 3 tried,
  newest first) has its `SHA256SUMS` checked against `SHA256SUMS.sig` with the keys the builder trusts, as a player's
  update checks it, then its full file by the SHA-256 the signed sums give, and only then is its pack read (appended,
  or compiled in by a release from before) and its `tree/third_party_objects` member written out, each file checked.
  `compile --third-party-objects` takes from that set (`LONGSHOT_THIRD_PARTY_OBJECTS`) each object whose key is this
  build's: the compiler's identity, the flags, the source and every header it read, the same key a player's build takes
  a prebuilt object by. Those objects are the last release's bytes, so the pack's member, and the deltas, stay as they
  were; any object whose inputs changed is compiled. Nothing to be had (no such release yet, a list that doesn't
  answer, a signature that doesn't check) is said (a warning when something is refused), and every object is compiled.
  This keeps the rule that nothing a release ships comes from a cache: the objects taken are a published release's own,
  signed with the release key and checked before a byte of them is used, never a runner's cache that anyone could
  have written.
- **The pack's members that did not change come from the last release too.** `last-objects` keeps that release's whole
  pack (`<out>/sources.pack`), read from the file its signed sums named, and `dev release sources --reuse` takes from it
  each member whose key is this build's: every member of a release's pack ends with a zstd skippable frame holding the
  SHA-256 of its raw bytes and of what compressed them (the zstd program's `-V` and its arguments, `MEMBER_ZSTD`), and
  one is taken only when that key is this member's and its frame, decompressed, is this member's raw bytes, compared
  whole. Those are the bytes compressing it again would give, so the pack is the same either way; the tools (llvm,
  the sysroot, cpm, ffmpeg, SDL2), most of its bytes, change only with `tools.lock`. The rest are compressed several at
  once, one single-threaded zstd each, as many as the runner's threads, and put in the pack in their order, so the
  bytes are those of one after another. The first release after the key frames came has none to take from (its last
  release's members carry no key) and compresses all of them, about 8 minutes on the release runner; after it, a
  release that changes no tool compresses its source members alone. Nothing to be had (no pack kept, one that can't be
  read) is said, and every member is compressed.
- **The program ships without debug info.** The dist profile (`Cargo.toml`: thin LTO, 16 codegen units) generates
  none (`debug = false`) and strips what the standard library brings (`strip = "debuginfo"`); the symbols stay, so a
  panic's backtrace names its functions, without files or lines.
- **The build jobs' artifacts are zipped stored** (`compression-level: 0`): what `dist/` holds is compressed already.
  The Linux tool, a plain program, is zipped at level 1.

## Making a release

You need maintainer rights to push tags, the remote `forge` (set up by Tooling's `forge.py login`), and the full
history (`git fetch --unshallow --tags` in a shallow clone). Agents: the
[release skill](https://git.magicbean.dev/MagicBeanDev/Tooling/src/branch/main/skills/release/SKILL.md)
(`/magicbean:release stable`) does these steps and writes the page's "What's new".

```sh
export GIT_CLIFF=$(python -I ../Tooling/scripts/release.py git-cliff "$(python -I scripts/dev.py paths tools)/git-cliff")   # the pinned git-cliff
python -I ../Tooling/scripts/release.py next       # the number the commits call for, the next stable's tag, its notes
python -I ../Tooling/scripts/release.py channels   # what each channel offers now
```

### A nightly

Nothing to do. A merge to `main` makes one, about 5 minutes after the last of several merges in a row (at least once
an hour while merges keep coming). When the newest nightly is an hour old or more, a merge's nightly starts at once,
with no wait. A push whose commits since the last nightly are only `ci:` or `chore:` makes none
(`release.py nightly-news`). If a nightly fails, read the failed step on the run (*Actions* > *Release*): fix code in a
pull request, or *Re-run* for a runner or network failure. Players keep the last good nightly meanwhile.

### A stable

A stable is a tag on a commit of `main` that testers played: normally a nightly's commit (the nightly channel is where
the next version is tried). There are no release candidates.

1. **Before a minor's stable**, if the renderer changed much, record the renderer's play profile again and merge it
   first, so the nightly testers play has it ([recording it](../developing.md#record-the-renderers-play-profile)). An
   old profile still builds.
2. **Preview** on an up-to-date `main`: `release.py next`. Read the notes: a wrong title is a wrong line, and the commit
   on `main` keeps the title it was merged with.
3. **Get it tested:** testers on the nightly channel, and a [Windows tester](#windows-tester) on the nightly to be
   tagged.
4. **Tag** the tested nightly's commit (`nightly-<count>-<commit>` names it; jj doesn't push tags, so tag with git in
   the colocated clone):

   ```sh
   git tag -a v3.1.0 -m "Longshot v3.1.0" nightly-412-1a2b3c4d^{}
   python -I ../Tooling/scripts/release.py check-tag v3.1.0   # optional: the same check CI runs
   git push forge v3.1.0
   ```

   Or in the web UI: *Releases* > *New release*, type the tag, target the commit, **Create tag** only. Never *Publish
   release* or *Save draft*: CI makes the release, and one made by hand has no files.
5. **Check it:** the run is green; the page has every file, `SHA256SUMS` and `SHA256SUMS.sig`; `release.py channels`
   shows the tag on `stable`; downloaded files pass `release.py verify SHA256SUMS --files <folder>`; a launcher on the
   stable channel offers it.

CI rebuilds it from that commit and publishes it on `stable`. Its notes cover everything since the previous stable.
Tagging a later commit works, but then it isn't what testers tried.

### A hotfix

Fix it on `main` in a `fix:` pull request. `release.py next` gives the patch (or the minor if a `feat` merged since).
Let its nightly be played quickly (on Windows too), then tag the stable on its commit; in an emergency, tag it
without waiting. Releases come only from `main`: revert anything on `main` not ready to ship, in a pull request, first.

### Pulling a bad release

1. On its release page, **Edit** and tick *This is a draft*, or **Delete** it. Its channel falls back to the version
   before at once.
2. Launchers that installed it keep it (they never downgrade by themselves). Tell players in an issue and the chat to
   build the previous version, or for the launcher, run `--rollback`.
3. Release the fix as the next number. Never move or reuse a tag: launchers that installed it wouldn't see the fixed
   files as an update.

### When CI refuses a tag

The *version* job says why, and nothing was built. Delete the tag (`git push forge :refs/tags/<tag>` and
`git tag -d <tag>`, or *Tags* in the web UI), then:

| The error says | Do |
|---|---|
| `is not vX.Y.Z` | a typo, or a number below `v3`: tag the right name |
| `is not on main` | tag a commit of `forge/main` |
| `is not above every release there is` | the number is taken or lower: `release.py next` gives the next one |
| `is lower than the commits since vX call for: at least vY` | a `feat` or breaking change came in: tag `vY` |
| `no tag ... here` | the tag isn't in this clone: `git fetch --tags`, or make the tag before checking it |

### Windows tester

CI only shows that the Windows files load. Before each stable, someone on Windows 10 or 11 tests the nightly to be
tagged:

- **Build and play:** with the launcher, build that nightly from their own copy and play to the title screen; then
  update to it from the nightly before, so the launcher installs it by its delta.
- **Note what Windows says:** nothing is signed, so record what SmartScreen and the antivirus do.
- **Run the Windows-only checks:** `python -I -B tests/run_all.py --strict` on Windows' Python (the checks Linux skips
  name why in their `RUN_ALL_SKIP`), and from a Visual Studio 2022 developer prompt
  `cl /nologo /std:c++17 /W4 /WX /EHsc gpu\vk\tests\present_rect_test.cpp && present_rect_test.exe` and
  `python -I -B tests/test_invalidate_vcore.py --cxx cl`.

A failure is an issue and blocks the stable.

## Release notes

git-cliff writes them at release time from the commit titles since the previous tag, with [cliff.toml](../../../cliff.toml)
(`release.py notes`). Headings: Features, Fixes, Performance, Reverted, Documentation, Code, Build, Tests; `ci` and
`chore` are left out. A stable's notes cover everything since the previous stable. A nightly after the first lists what
is new since the nightly before, then *Earlier changes*, everything since the last stable. The notes go on the release
page only.

## Signing

CI signs `SHA256SUMS` with the release key (ECDSA P-256, the organization secret `RELEASE_SIGNING_KEY_SEALED`, opened
only on the server's runner: [CI](ci.md#secrets-only-our-runners-can-open)), then checks
the signature against the public keys the launcher trusts (Longshot's [release_keys.json](../../../scripts/release_keys.json),
the launcher's `Core/ReleaseTrust.cs`): a key the launcher doesn't trust stops the release in CI. The launcher checks
the signature, then each file it downloads against `SHA256SUMS`, and a file it made from a delta against the
manifest's SHA-256 of the full file. `longshot-builder` checks every file of its pack against its SHA-256 as it unpacks it.
`LONGSHOT_LAUNCHER_TRUST=<public .pem>` makes a launcher trust one more key, for trying a test release; `longshot-builder`'s `fetch` and `self-update` read it too, and `LONGSHOT_TRUST` for the builder alone (`release::trust::TRUST_VARIABLES`). Never set either for players.

Who can sign: anyone who can push a branch to this repository can run a workflow on the deploy runner and unseal the
release key and the packages token ([CI](ci.md#secrets-only-our-runners-can-open)): push access is release-signing
access, until the deploy runner takes only `main`'s and the tags' jobs.

Uploads go through `RELEASE_UPLOAD_API` (an organization variable, passed to `release.py` as `FORGEJO_UPLOAD_API`): the
API without Cloudflare in front, which cuts uploads at 100 MB. The build jobs' artifacts (`longshot-builder-<platform>`, the Linux
tool) and the CI images' layers are uploaded the same way first: `upload-artifact` with `ACTIONS_RESULTS_URL` set to
that host; when that fails, or the variable is unset, through the public address, then once more. (If Forgejo answers
with upload addresses under its `ROOT_URL`, the bytes still go through Cloudflare: the run's log shows which.) Releases are made with the job's own token; no other
secret is needed.

### Rotating the key

When it may have leaked, or on a schedule:

1. `python -I ../Tooling/scripts/release.py keygen <safe folder>` makes `release.pem` and `release.pub.pem` and prints the lines.
2. **The launcher first:** add the new key beside the old one in `Core/ReleaseTrust.cs` and `scripts/release_keys.json`,
   and release the launcher **signed with the old key**.
3. **Then switch,** once most players have that launcher: replace `RELEASE_SIGNING_KEY_SEALED` with the new
   `release.pem`, sealed (`scripts/ci_secret.sh seal --key seal.key < release.pem`),
   remove the old key from both files, and release the launcher with the new key.

Older launchers then refuse new releases and the player downloads the launcher by hand. If the old key leaked, do
steps 2 and 3 at once and tell players.

## tools.lock

`longshot-builder` carries its build tools: [tools.lock](../../../tools.lock) names each one, a line each. No player
downloads a tool.

```text
tool <name> <version> <system> <sha256> <size> <dest> <prefix> <url> [<url> ...]
```

`system` is `windows-x64` or `linux-x64`; `dest` is where the tool is unpacked under the builder's tree (`tools/<name>`);
`prefix` the archive's folder that is the tool (`-`: the whole archive); the URLs are Longshot's store, a file
`<sha256>-<name>.tar.zst` of the organization's generic package `<name>-<system>`, version `<version>`, from
`git-cdn.magicbean.dev` (Cloudflare's edge), then `git.magicbean.dev`. A package version's files never change and are
never pruned.

| System | Tools | Download (zstd), unpacked |
|---|---|---|
| Linux | `llvm` (clang and lld for x86-64, clang's headers, the builtins and profile runtimes), `sysroot` (glibc 2.35, GCC 13's runtime), `ffmpeg` (static, with dav1d), `sdl2`, `cpm` (Vulkan, SPIR-V and VMA headers) | 113.5 MB, 520 MB (`llvm` 96.3 MB, 407 MB) |
| Windows | `llvm-mingw` (clang and lld for x86_64-w64-mingw32: its MinGW-w64 headers and libraries, clang's headers and runtimes), `cpm` (the headers, dav1d, FFmpeg built with llvm-mingw), `sdl` (`SDL2.dll`, `SDL3.dll`) | 58.0 MB, 410 MB (`llvm-mingw` 49.8 MB, 314 MB; upstream's zip is 191 MB, 750 MB) |

Each archive is made from pinned sources and releases, and comes out the same every time. Everything that makes them
is in [builder/tools/](../../../builder/tools/make.sh), apart from what uses them: `pins.sh` holds every upstream pin (a
version and its SHA-256), one recipe per tool makes its folder (`linux-x64/llvm.sh`: LLVM 23.1.5's release trimmed by
its `trim_llvm`; `linux-x64/ffmpeg.sh`, `sdl2.sh`, `cpm.sh`, `sysroot.sh`; `windows-x64/llvm-mingw.sh`: llvm-mingw's
release trimmed by `trim_llvm_mingw`; `windows-x64/cpm.sh` with `dav1d.sh`, `sdl.sh`), and `make.sh` runs them and packs
each with `longshot-builder dev tools make`: sorted, every time and owner fixed, zstd 1.5.7 at `-19 --long=27`. `lib.sh`
builds that zstd and the nasm FFmpeg and dav1d are assembled with (2.16.03) from their pins, so a PC's own changes
nothing. `docker.sh` runs `make.sh` in `builder/tools/Dockerfile`'s Ubuntu 22.04 on any distribution.
The sysroot is the build system's files of the day: its version carries the date, and it comes out the same twice only
within a day on one system, which is what the Make tools workflow checks.

**The compiler is LLVM's own release, trimmed.** Linux's `llvm` is 96.3 MB compressed and 407 MB unpacked: its
`clang-23` is LLVM's static build, which carries every LLVM target. A clang and lld of our own, built from the pinned
LLVM source with only the targets the build needs (X86, plus AArch64 for the synthetic-game test) as LLVM's single
multi-call driver, would be about half that: an estimated 40-50 MB compressed and 170-190 MB unpacked. It is planned
as a recipe of `builder/tools/` that builds it the same
every time, made and uploaded by the Make tools workflow like the other tools, `tools.lock` then naming
it. Its cost: an LLVM build of 1.5 to 3 hours in CI on each LLVM bump. Not done yet.

The release puts every tool in its `longshot-builder` (`longshot-builder dev tools install <bundle> --system <s>` unpacks each at its dest,
`tools/<name>`; `longshot-builder dev release sources --tools <bundle>/tools` packs them in), each a member of the pack of
its own. At the first use each is unpacked once into `<cache>/tools/<name>-<hash>/` and linked into the unpacked tree as
`tools/<name>` (build-driver's `sources.rs`), so a release with the same tool unpacks nothing of it, and its delta
carries nothing of it. The compile takes them from there (build-driver's `tools/`; [build.md](build.md#the-build-tools)):
nothing of the player's PC stands in for them, not even its own `RX_CPM` or `LONGSHOT_MINGW`. A developer installs them
into a checkout the same way (`mise run tools`, or `cargo run --release -p longshot-builder -- dev tools install .
--system linux-x64`: `tools/`, git-ignored), or lets `dev.py` install
them into its `tools-store/`, shared by every checkout (named with `LONGSHOT_OWN_TOOLS=1`). `longshot-builder stamps
[--output json]` prints the compiler's stamp (of the compiler, sysroot and `cpm` lines) and the shader translator's, which a
launcher keys shader caches by (the [`stamps` event](build-events.md#the-events)); a build writes both into the game
folder's `longshot_version.txt`.

### Bumping a tool

Using the tools needs none of this: `tools.lock` and `longshot-builder dev tools install` (`mise run tools`) are all CI
and developers run. Making them is for whoever bumps a pin. A bump is two pull requests:

1. **The pin.** Change it (the version and its SHA-256) in `builder/tools/pins.sh`, and its recipe if what it keeps
   changes, with its `LLVM_TRIM` or `LLVM_MINGW_TRIM` for a trimmed toolchain. For llvm-mingw, also
   `LLVM_MINGW_LINUX_SHA256` there, the same release's Linux build CI cross-builds with (`scripts/longshot_builder.py
   llvm-mingw` reads both; `tests/test_windows_on_linux.py` checks tools.lock's line is that release). Try it locally
   with `builder/tools/docker.sh --check <system>/<name>` (Ubuntu 22.04 in a container; `builder/tools/make.sh` on an
   Ubuntu 22.04 PC), and on the pull request's branch with the Make tools workflow, which makes it twice and uploads
   nothing off `main`. Check nothing made from the game is in it, and build the game with it: `make.sh` writes the
   `longshot-builder dev tools add ... --no-upload` lines that put it in a checkout's `tools.lock` to `add.sh` beside
   the archives (`build-linux/tools-made/`), then `mise run tools` and a build.
2. **The archive.** Once the pin is on `main`, start the Make tools workflow there (*Actions* > *Make tools* > *Run
   workflow*, its input `<system>/<name> ...` or `all`). Its make job makes each archive with its recipe in Ubuntu
   22.04, makes it again from nothing but the downloads and stops unless the two are the same bytes; its publish job,
   on the deploy runner and on `main` only, uploads them as BeanTools with the sealed `PACKAGES_TOKEN_SEALED`
   (`scripts/ci_tools_publish.py`, Python's standard library: it compiles and runs nothing of the checkout's), checks
   each is in the store, and gives `tools.lock` with their lines in the run's summary and as the artifact `tools-lock`.
3. **The line.** Commit that `tools.lock` in a pull request (`build(windows): llvm-mingw 20261201`). The next release
   carries the new tool.

A package version's files never change, and are never pruned: an archive whose version `tools.lock` names already must
be that line's bytes (`make.sh` and `ci_tools_publish.py` refuse another), so a recipe that keeps other files takes a
new version (its `*_TRIM`). Every line's first two URLs must be the store's (`dev tools check` refuses any other), and
before any build the release checks every line's archive is there with its size and SHA-256 (`dev tools check
--require`, in the version job): a release only downloads tools. A line whose archive is not there stops it with "a
tools.lock line's archive is not in Longshot's store": run the Make tools workflow for it. Every one of these is the
builder's (`longshot-builder dev tools`, the one reader of `tools.lock`: build-driver's `tools/`); each job that runs
one builds a `longshot-builder` first, which needs none of the tools to be built. In an emergency an archive can be
uploaded by hand: `dev tools add` without `--no-upload`, with `RELEASE_TOKEN` (`write:package`) and, past 100 MB,
`FORGEJO_UPLOAD_API`. Play profiles are not tools: they are committed in `assets/profiles/`.

RenHDrum isn't a tool either. The release builds `renhdrum.dll` and `renhdrum.so` from `third_party/renhdrum`
(`scripts/renhdrum.py build` in `build-windows.yml` and `build-linux.yml`, with a pinned Rust, its build crate by its
`Cargo.lock` SHA-256 and libopus from `third_party/opus`) and packs them into `longshot-builder`'s tree
(`longshot-builder dev release sources --add`), where a player's build finds them. A newer RenHDrum: its `PROVENANCE.md` says how. A newer Rust:
`RUST_VERSION` and the parts' SHA-256 in `scripts/renhdrum.py`.

## Update channels

The launcher reads the channels from the repository's list of published releases, by the same rules as
`release.py channels`:

| Channel | Offers |
|---|---|
| `stable` | the newest published `vX.Y.Z` |
| `nightly` | the newest published `nightly-*`, by its count |

A draft is on no channel, and so is an old `vX.Y.Z-rc.N`. Each build in the launcher follows its own channel, and the
launcher updates itself from a channel of its own (*Settings*). It never moves a build or itself to a lower version;
going down is the player's choice. The launcher installs each `longshot-builder` with
[`longshot-builder fetch`](build.md#fetch-a-version) and starts it with `--no-self-update`; how it updates itself is
documented in [LongshotLauncher](https://git.magicbean.dev/MagicBeanDev/LongshotLauncher). A copy of `longshot-builder`
no launcher manages (RecompLauncher's zip, a version folder of its own) updates itself on its channel, after asking the
player once ([self-update](build.md#self-update)). The game doesn't look for updates.

### Deltas, applied

The builder applies them (the release crate, `builder/crates/release`; the launcher's C# did before):

- **One delta**: the first the manifest lists whose `from_sha256` is the SHA-256 of a program the launcher keeps
  (`fetch --from <version folder>`, each folder's `longshot_release.json` naming the full file it is; a folder without
  one has its program hashed, a windowed Windows program read as the console one it was made from).
- **A chain**, when none fits: up to 5 deltas one after another, through the releases between, each intermediate file
  checked against its own release's manifest (signed in its own `SHA256SUMS`) and deleted once the next is made. It
  passes only through the channel's 20 releases before the target (5 deltas back from each, 4 releases passed
  through), and for a nightly the latest stable; the fewest hops win, then the smallest download, and a chain not
  smaller than the full file is never taken.
- **Applied** as `zstd -d --long=30 --patch-from=<base>` does it, by ruzstd (no zstd program needed); a delta whose
  window is past 2^30 is refused.
- **Anything that fails** (a download, a delta that doesn't apply, a result of another SHA-256) falls back to the full
  file; a full file that fails is the error.

A release of the earlier kind (make_game.py's: no `longshot-builder-<v>-<platform>.json`) is refused by `fetch` with
`no_builder_in_release`.

**Download links.** `https://longshot.magicbean.dev/download` (served by `website/worker/index.ts`) points to the newest
launcher: `/download/launcher/<os>` for the best channel there is (stable, else nightly),
`/download/launcher/<channel>` for a channel's release page, `/download/launcher/<channel>/<os>` for its file.
`<channel>` is `stable` or `nightly` (an old `rc` link goes to `stable`); `<os>` is `windows`, `windows-portable` or
`linux`. There are no links to `longshot-builder`'s files: only the launcher downloads them.

## Pull requests' and main's builds

A pull request's head, or main's newest commit, has no release. The launcher builds one on the player's PC with the
newest nightly's `longshot-builder from-source --commit <head> --out <version folder> --name pr-<number>-<commit's
first 8>` ([a builder from source](build.md#a-builder-from-source)): it downloads that commit's archive by its full hash
and compiles the commit's own `longshot-builder`, with the Rust, crates and build tools the commit pins, into a version
folder like a release's (the source, the third-party objects, RenHDrum's library and the tools packed inside). A pull
request may change the builder itself (the lifter, the compile), so the nightly's builder never builds a pull
request's game: it builds the pull request's builder, which then builds the game as any release does.

Nothing about it is signed or checked against a release: the launcher shows its warnings first, and the version folder's
`source.json` says `"signed": false` and the commit. A tool the commit pins with the nightly's SHA-256 is the nightly's
own copy, so a pull request that leaves `tools.lock` alone downloads only its archive (about 25 MB), Rust (about 120 MB,
once) and the crates (about 55 MB, once). CI builds no pull request's files for players.

## RecompLauncher

[RecompLauncher](https://recomplauncher.com) can install and build Longshot through `recomplauncher.json`
(`tests/test_recomplauncher_json.py`). A release carries it in `longshot-builder-<v>-<platform>-recomplauncher.zip`, made
by each platform's build job (`longshot-builder dev release recomplauncher`, the job's Linux builder, right after
`dev release files`) from the platform's full file, and uploaded with it: `SHA256SUMS` names both. RecompLauncher only finds releases on GitHub, so a
player adds the zip to it by hand.

RecompLauncher's [Recomp Standards](https://github.com/elripalda/RecompLauncher/blob/main/docs/DEVELOPERS.md#recomp-standards)
ask for one program, the one players start, that builds the game when it must and then runs it, and that is not named
a builder: with Easy Setup off, and from a shortcut or a Steam entry, RecompLauncher starts the release's program with
its `start` arguments. So the zip holds longshot-builder once, as `Longshot.exe` on Windows and `Longshot.x86_64` on
Linux, and `start` and `prepare` both run it. The install folder is a version folder
([play](build.md#play-the-version-folder-and-the-profile)), and the player's profile is `{config}/Longshot`
(`%APPDATA%\Longshot` on Windows, where the game kept its saves before; `~/.config/Longshot` on Linux):

| Where | What it is |
|---|---|
| `{install}/Longshot.exe` / `Longshot.x86_64` | longshot-builder: the program players start, and the build step |
| `{install}/recomplauncher.json` | this profile of RecompLauncher's |
| `{install}/builds/default/` | the game folder the build makes: the game (`longshot`, `longshot.exe`), its record, `logs/` |
| `{install}/cache/` | the builder's cache (tools, work folders, stores), the version folder's own |
| `{config}/Longshot/profile.json` | the player's profile: the copy (`game`) and the upscalers, beside the Mod API's keys |
| `{config}/Longshot/settings.ini` | the game's settings (from `LONGSHOT_PROFILE`): RecompLauncher's `config` |
| `{config}/Longshot/saves/` | the game's saves (from `LONGSHOT_PROFILE`): `save/`, `save-work/` |

- **Play** (`start`: `Longshot.exe --profile {config}/Longshot`): longshot-builder plays (`src/play.rs`,
  `build_driver::play`): the fast check, `update` only when it says so (the log in
  `builds/default/logs/longshot_update.log`), then `builds/default/longshot` with every other argument and
  `LONGSHOT_PROFILE` in its environment (the game's saves are the profile's `saves/`, its settings its `settings.ini`). It waits for the game and its exit status is the game's; a game that asked to be built again over its channel and exited 0 is an update and a new start. A profile with no copy and none chosen exits 120; a build or a
  start that failed exits 121; a second start of a profile whose game runs (or is being built) exits 123 with
  `--output text` (the profile's lock, [one copy per profile](build.md#one-copy-per-profile)). An update that failed of a folder that has its game says why and then starts the game it
  had (the swap puts in all of a build or none, so that game is whole). Both are said in a message box (Windows; zenity
  or kdialog on Linux, when there) when the output is `user` (never in RecompLauncher's Big Picture mode:
  [output](build.md#output)), and `exit_codes` explain them; `folders.logs` is the game folder's `logs/`.
  RecompLauncher's placeholders are taken in `start.args` as in `prepare.args`; should a
  RecompLauncher take none there, `start` with no arguments still plays, with `profile/` beside the program.
- **No console on Windows**: the zip's `Longshot.exe` is the full file with its PE subsystem made a windowed one's (2,
  not 3: `windowed` in `release_cmd.rs`), so nothing opens a console when it starts, and it gives what it starts (the
  compilers, the shader preparation) a hidden console of its own (build-core's `hidden_console`). Started from a
  console (a developer's terminal), it joins that one instead (`AttachConsole(ATTACH_PARENT_PROCESS)`), so what it says
  shows there. Its standard handles stay the ones it was given, so `prepare`'s lines still reach RecompLauncher and the
  game, given none, writes its log file. That one field is all that differs from the full file. The release's main
  file (`longshot-builder-<v>-windows-x64.exe`) stays a console program: developers, CI and a launcher reading `--output json`
  run it as one, and a console program's start waits in a terminal while a windowed one's returns at once. A launcher
  that makes a version folder players start puts the windowed `Longshot.exe` there: the RecompLauncher zip's, or the
  main file with that one field (2 bytes at the PE header's offset + 92) set to 2.
- **The game files** are its input `game`: the .nup (or a dump's folder), checked by its build ID. The game reads the
  .nup where it is, so it must stay there; nothing of it is copied, and there is no `keeps_copy`.
- **The build** (`prepare`) is `Longshot.exe build --game <copy> --profile {config}/Longshot --output recomplauncher` (a builder
  command, so by that name too it builds): the profile's `profile.json` written (its copy and upscalers), then
  `{install}/builds/default` built with the stamp the fast check compares.
  RecompLauncher reads `progress <0..1> <the step>` and `error <text>` lines on stdout (`--output recomplauncher`, build-core's
  `Format::Lines`; the build to 0.9 of the bar, the shader preparation the rest), and every other line is the log's.
  The work folder and the stores are in the version folder's `cache/` (the program carries its pack: [where things
  are](build.md#where-things-are)). A build of a folder it built before keeps what did
  not change, as `update` does. The ticks add `--upscaler dlss` with `--accept-licenses nvidia`, and `--upscaler xess`
  with `--accept-licenses intel` on Windows: longshot-builder fetches those files itself. The build is finished when
  the folder has its record, `builds/default/longshot_build.json` (`done`).
- **Saves** are the save folders only: the profile's `saves/save`, and the save folders and mouse sensitivity of the
  per-user place, where the game kept its saves before and keeps the mouse's sensitivity still (`{local}/longshot`,
  `{config}/Longshot`), never the whole data folder: on Linux, `~/.local/share/longshot` also holds the NFC tags, NVIDIA's files and the save backups.
- RecompLauncher builds the game again whenever the release it installed changes, so the file ships as it is, with
  nothing stamped per release. It needs RecompLauncher 0.90 Beta 68 or newer (an input's `check`).

What its format cannot say: where the program comes from (RecompLauncher installs the zip it was given: there is no
download of a release's program by its checksum, nor a delta), and a structured event stream (it reads the lines above,
not `--output json`'s events).

## One-time setup

What a repository needs before its first release (the organization's secret is shared by Longshot and the launcher):

1. **The key.** On a PC you trust, `python -I ../Tooling/scripts/release.py keygen <safe folder>`. Put the printed line in the
   launcher's `Core/ReleaseTrust.cs` and in [`scripts/release_keys.json`](../../../scripts/release_keys.json), a pull
   request to each. Keep `release.pem` backed up offline: losing it means a launcher with a new key that every player
   downloads by hand.
2. **The secret** (MagicBeanDev > *Settings* > *Actions* > *Secrets*): `RELEASE_SIGNING_KEY_SEALED`, `release.pem`
   sealed with the server's `seal.key` (`scripts/ci_secret.sh seal --key seal.key < release.pem`; [CI](ci.md#secrets-only-our-runners-can-open)).
   No token secret or bot account: `release.yml` makes the release with the job's own token, which Forgejo
   gives write access to its own repository on a push or a tag. Don't protect `nightly-*` tags, or the prune can't delete
   them. `longshot-builder dev tools` and `release.py` run by hand (`add`, `publish`, `prune`) take your own token as
   `RELEASE_TOKEN`.
   `PACKAGES_TOKEN_SEALED`: the token of BeanTools (an account in a team with *Packages: Write* alone, `write:package`),
   sealed the same way; the Make tools workflow's publish job opens it on the deploy runner only.
3. **The upload address.** Uploads through Cloudflare are cut at 100 MB, and the Linux `longshot-builder` is larger. Set the
   organization variable `RELEASE_UPLOAD_API` (passed to `release.py` as `FORGEJO_UPLOAD_API`, since Forgejo refuses
   variable names that start with `FORGEJO_`) to the API without Cloudflare, as the runners reach it. Unset, uploads go
   to `https://git.magicbean.dev/api/v1`.
4. **The attachment limits** in Forgejo's `app.ini`: `[attachment] MAX_SIZE` at least the largest file, and
   `[repository.release] ALLOWED_TYPES` empty, so `.exe`, `.zst-patch`, `.json`, files without an extension and
   `SHA256SUMS` are accepted.
5. **Actions and runners:** Actions enabled on the repository, and Linux runners registered (the Windows files are
   cross-built on Linux; the runners are in [CI](ci.md#runners)).
6. **The tools' packages:** every line's archive in the store (the Make tools workflow uploads them), so that
   `longshot-builder dev tools check --require` passes.
7. **Try it** on a fork, or with a throwaway key: `LONGSHOT_LAUNCHER_TRUST=<test .pub.pem>` makes a launcher accept a
   release signed with it.

## Troubleshooting

| Symptom | Fix |
|---|---|
| *SHA256SUMS, signed*: "the sealed secret is empty" | `RELEASE_SIGNING_KEY_SEALED` isn't set or visible to the repository ([one-time setup](#one-time-setup), step 2) |
| *SHA256SUMS, signed*: "the seal broker gave no key (refused: ...)" | the broker's reason follows: a run that isn't main's or a `v*` tag's, or another repository ([CI](ci.md#secrets-only-our-runners-can-open)) |
| *SHA256SUMS, signed*: "the seal broker gave no key ()", or curl can't connect | the job ran on a runner that can't reach the broker (only the server's quick runner should carry `deploy`), or `SEAL_BROKER_PIN` doesn't match the broker's certificate |
| *SHA256SUMS, signed*: "no ID token" | the publish job lost `enable-openid-connect: true` |
| *SHA256SUMS, signed*: "bad decrypt" | the secret was sealed with another `seal.key`: seal it again with the server's |
| `verify`: "not signed by a trusted key" | the secret's key isn't in `release_keys.json`: set the matching key, or rotate |
| *Publish*: HTTP 413 or a cut connection | an upload over Cloudflare's 100 MB limit: set `RELEASE_UPLOAD_API` |
| *Publish*: "upload N of 4 failed", HTTP 5xx | `release.py` tries each upload four times; if all fail, check Forgejo's log and re-run (the draft is reused) |
| *Publish*: HTTP 400/422 | Forgejo's attachment size or type limits (`[attachment] MAX_SIZE`, `[repository.release] ALLOWED_TYPES` empty) |
| *Publish*: HTTP 401/403 | the run isn't a push to this repository: release from a tag or push here only |
| "is already published" | a re-run of a released version: nothing to do |
| *The version*: "tools.lock is missing" | [bump the tool](#bumping-a-tool) |
| *The version*: "a tools.lock line's archive is not in Longshot's store" | run the Make tools workflow on `main` for that tool, or fix the line ([bumping a tool](#bumping-a-tool)) |
| *Make tools*: "is not made the same twice" or "A package version's files never change" | the recipe's output depends on something unpinned (a time stamp, a path, the system's own tool), or its files changed under a version already in the store: fix the recipe, or give it a new version (its `*_TRIM`) |
| Too many nightlies | the prune failed on a protected `nightly-*` tag: unprotect it, or `RELEASE_TOKEN=<token> release.py prune` |
| `next` or `notes`: "git-cliff is needed" | `export GIT_CLIFF=$(python -I ../Tooling/scripts/release.py git-cliff <folder>)` |
| A launcher offers nothing on a channel | `release.py channels`, then check that release's files and signature |
| A draft is left after a failed run | re-run it (the draft is reused), or delete the draft |
| The launcher: "not signed by a key this launcher trusts" | a release signed with a key the launcher lacks: release the launcher with the key first ([rotating the key](#rotating-the-key)) |
| A launcher doesn't see a new release | it checks at start and on Retry, and never sees a draft: check the publish finished |
| A launcher's update falls back to the full file | the delta did not apply or its result's SHA-256 was not the manifest's (`fetch`'s `log` events say which); the full file is the answer, nothing to fix unless it happens to every player |
| `fetch`: `no_builder_in_release` | the release has no manifest for the platform: a make_game.py release, which `fetch` doesn't install |

## Tools

| Tool | What it does |
|---|---|
| `python -I ../Tooling/scripts/release.py` | versions (`next`, `check-tag`, `nightly-name`), notes, sums and signatures (`sums`, `sign`, `verify`, `keygen`), publishing (`publish`, `prune`, `released-as`, `channels`); `--help` lists all |
| `longshot-builder dev tools` | `tools.lock`: `check`, `install`, `add`, `make` (`cargo run --release -p longshot-builder -- dev tools ...`; `mise run tools` installs this system's into the checkout) |
| `python -I scripts/longshot_builder.py` | what must run before there is a builder: `longshot-builder` per platform with the pinned Rust (`build`), its headers checked (`check`), the pinned llvm-mingw that links the Windows one (`llvm-mingw`) |
| `longshot-builder dev release` | the release's packaging, by a builder already built: the last release's third-party objects (`last-objects`), the pack it carries (`sources`, `unpack`: build-driver's `sources.rs` writes and reads it; `attach`, which appends it to a linked program and reads it back), a player's build's dry run (`dry-run`), the earlier releases' files (`fetch-bases`), the deltas and the manifest (`deltas`; `files`: the stamps, `fetch-bases` and `deltas` together), RecompLauncher's zip (`recomplauncher`); `dev release --help` |
| `builder/tools/make.sh`, `docker.sh` | the build tools' archives, each made from its pins by its recipe (`make.sh linux-x64/sdl2 ...`, `all`; `--check` makes each twice); `docker.sh` in an Ubuntu 22.04 container |
| [cliff.toml](../../../cliff.toml) | git-cliff's settings for the release notes |
