# Setup

How to take a PC from nothing to a built game folder: the tools, the team's Tooling repository, the compiler for your
OS, and a first build from your own dump. The daily loop after that is in [developing](developing.md).

**No game files are in this repository, and nothing built from your dump leaves your PC**: not a game folder, a base,
a pack, a cache or a profile ([the rule with no exceptions](../../CONTRIBUTING.md#the-rule-with-no-exceptions)).
Everything a build makes from the dump goes to its `--out` folder, its work folder and the stores: with `mise run
build`, `work/` in your development folder (`~/.local/share/longshot-dev`, `%LOCALAPPDATA%\longshot-dev` on Windows;
[where dev data lives](reference/build.md#where-dev-data-lives)); with `longshot-builder` alone, the per-user cache
(`~/.cache/longshot-builder`, `%LOCALAPPDATA%\Longshot\Builder`), or `--work`.

## 1. Get the code and set it up

1. Install [mise](https://mise.jdx.dev/getting-started.html). It pins every tool the repository uses.
2. Clone the repository. A shallow clone is enough for everything but cutting a release and naming a build by its tag:

   ```sh
   git clone --depth 1 https://git.magicbean.dev/MagicBeanDev/Longshot.git     # jj: jj git clone --colocate --depth 1 ...
   ```

   `git fetch --unshallow --tags` once gives you the history (`git log`, `git blame`, version names, and Tooling's `scripts/release.py`).
3. In the checkout, run `mise install`, then `mise run tools` and `mise run setup`. `mise run tools` puts the build
   tools `tools.lock` pins (the compiler, the sysroot, FFmpeg, SDL and the headers, made by CI from their pins) in the
   checkout's `tools/`, as a release's `longshot-builder` carries them ([section 3](#3-set-up-the-compiler)): with
   mise.toml's tools, that is everything a build needs. `mise run setup` does what it can without an account,
   ends with a list of what it did and what is left for you, and is safe to run again:

   | Step | What it does |
   |---|---|
   | Tooling | Clones [MagicBeanDev/Tooling](https://git.magicbean.dev/MagicBeanDev/Tooling) beside the checkout as `../Tooling`, or updates it with `git pull --ff-only`. A jj repository or a detached HEAD there is left alone |
   | Hooks | Installs the pre-commit and commit-msg hooks |
   | Checks' tools | On Linux, fetches the pinned Vulkan and SPIR-V headers and glslang that the game-free checks use, and installs their pinned Python packages into mise's Python (the same pins as `requirements.txt`) |
   | Claude Code | If `claude` is installed and the `magicbean` plugin isn't, prints the two commands to install it |
   | Rust | Checks that `cargo` is mise.toml's pinned Rust, which compiles longshot-builder and, in a developer's game build, the game's Rust (`longshot-rs`, for Linux or `x86_64-pc-windows-gnullvm`; a player's build links the release's prebuilt library) |
   | Build tools | Says whether `tools.lock`'s build tools are in the checkout's `tools/`, and `mise run tools` when they aren't |

4. **Sign in** once, from this checkout: `python -I ../Tooling/scripts/forge.py login`. Click **Sign in with Discord**
   and approve. The one sign-in covers git on every repository (the remote `forge` points at Longshot), the team chat
   and Test Farm requests. Setup never signs in for you.
5. **Set up pushing:** `python -I ../Tooling/scripts/forge.py fork` makes your fork and the remote `forge-fork`
   (`pr create` does it too if you haven't).

`mise tasks` lists the tasks: `setup`, `build` (`longshot-builder` with your arguments), `build-dev` (its `compile`
alone), `tools` (tools.lock's build tools into the checkout), `rust` (`builder/`'s checks: rustfmt, clippy, `cargo test`),
`dev`, `test`, `test-changed`, `lint`, `ci` (what CI checks on every change) and `tracy-tools`. Run any Python script from the checkout root with `python -I` (or `mise exec -- python -I ...` for the pinned
interpreter). Python is for the development tools and the checks only: the builder is Rust, and no player needs Python.

## 2. Get the team's tooling

The sign-in, `forge.py` (forks, pull requests, issues, reviews), the team chat and the agents' skills are shared by every
MagicBeanDev repository and live in [MagicBeanDev/Tooling](https://git.magicbean.dev/MagicBeanDev/Tooling), which
`mise run setup` clones as `../Tooling` (elsewhere: use that path wherever these pages say `../Tooling`). Its README and
[docs/forge.md](https://git.magicbean.dev/MagicBeanDev/Tooling/src/branch/main/docs/forge.md) are the reference.

Keep it current at the start of each session: `mise run setup` again, or `git -C ../Tooling pull --ff-only`.

Run Tooling's scripts from this checkout; never copy them in:

| Script | What it is for |
|---|---|
| `../Tooling/scripts/forge.py` | `login`, `fork`, `search`, `pr create/update/ready/wip/status/view/review/merge`, `issue ...` ([docs/forge.md](https://git.magicbean.dev/MagicBeanDev/Tooling/src/branch/main/docs/forge.md)) |
| `../Tooling/scripts/team_chat.py` | Read and post in the team's Matrix room, `#agent-updates:magicbean.dev` |
| `../Tooling/scripts/workspaces.py` | jj workspaces of the repositories beside it: `new`, `list`, `clean`, `remove` |
| `../Tooling/scripts/farm_run.py` | A Test Farm run of two commits or nightlies outside a pull request: `submit` and `cancel` (BigBeans), `status` and `wait` (anyone) |

### Claude Code: the `magicbean` plugin

Tooling is a Claude Code plugin marketplace. Install it once per PC (`mise run setup` prints these when it is missing):

```
/plugin marketplace add https://git.magicbean.dev/MagicBeanDev/Tooling.git
/plugin install magicbean@magicbean
```

`claude plugin list` shows `magicbean@magicbean` once it is in; `/plugin marketplace update magicbean` updates it.
Other agents read `../Tooling/skills/<name>/SKILL.md`. Longshot's own skills are in `.claude/skills/` and load by
themselves in this checkout.

| Skill | Use it to |
|---|---|
| `/magicbean:team-updates` | Read the room, then post a claim, finding, pull request, blocker or finished task |
| `/magicbean:review` | Review a pull request, a ranked pass over the open ones, or your own branch cold |
| `/magicbean:release` | Cut a stable ([releases](reference/releases.md)) |
| `/magicbean:farm-run` | Queue a Test Farm comparison of two commits or nightlies (BigBeans) |
| `prior-work` (Longshot) | Find pull requests, issues, claims and Hookshot changes that already do what you plan |
| `fix-with-test` (Longshot) | Fix a bug as a game-free test that fails on `main`, then the fix |
| `measure` (Longshot) | Measure a change or a switch in the game against `main` and write up the numbers |

## 3. Set up the compiler

The build makes two modules: the game (`longshot.exe`, `longshot` on Linux) and the Vulkan renderer (`longshot_vk.dll`,
`longshot_vk.so`). Every build compiles the renderer, so its headers and clang++ are always needed. The builder itself,
`longshot-builder`, is Rust 1.99.1, which `mise install` puts in (pinned in `mise.toml`, with rustfmt, clippy and the Windows
target, `x86_64-pc-windows-gnullvm`, for `cargo clippy --target`). `dev.py`'s builds and `mise run build` compile it
for you into your development folder's `cargo/`, from the
checkout they build (`LONGSHOT_CARGO` names another cargo; `LONGSHOT_BUILD`, a ready-made `longshot-builder`); by hand,
`cargo build --release -p longshot-builder`. CI and the release use rustup's 1.99.1 or `scripts/longshot_builder.py build` (the parts
`scripts/renhdrum.py` pins).

### The compile cache

Recommended for anyone who builds in more than one checkout or jj workspace: [sccache](https://github.com/mozilla/sccache)
as cargo's wrapper, one compile cache for every tree on the PC. A new workspace then takes its crates.io dependencies,
most of a cold build, from the cache; the repository's own crates compile per workspace, since their paths differ.
Install it once for your user and turn it on in your shell's profile (it is not in `mise.toml`, so CI and checkouts
without it are unchanged):

```sh
mise use -g aqua:mozilla/sccache@0.19.0     # or your package manager's sccache
export RUSTC_WRAPPER=sccache                # in ~/.bashrc, ~/.config/fish/config.fish (set -gx ...), or Windows' user env
export SCCACHE_CACHE_SIZE=40G               # its default is 10 GB
```

`sccache --show-stats` shows the hits; `RUSTC_WRAPPER=` (empty) turns it off for one shell. Agents sharing a PC set the
same in their jobs, so they share one cache.

- Give each checkout or workspace **its own target dir**. Never point two at one `CARGO_TARGET_DIR`: cargo names a
  path crate relative to its workspace, so one tree's build would pass for another's, and a check could pass on code it
  never read.
- Incremental builds (the `dev` profile) are compiled as usual: sccache leaves them alone and caches the rest.
- The game's C and C++ are not cargo's: the builder's content-addressed compile cache keeps them.

**The quickest way to get exactly a player's tools** is to install `tools.lock`'s build tools into the checkout:
`mise run tools` (`cargo run --release -p longshot-builder -- dev tools install . --system linux-x64`, or `windows-x64`) unpacks each at `tools/<name>`
(git-ignored), where a release's `longshot-builder` unpacks its own, and `longshot-builder --root .` then compiles with them as a player's
build does ([the build tools](reference/build.md#the-build-tools)). Or `python -I scripts/dev.py env`: it installs
them once in your development folder's `tools-store/`, shared by every checkout, and prints the environment that names
them (`LONGSHOT_OWN_TOOLS=1` with each tool's variable), for your shell to apply. `dev.py`'s builds (`base`, `pack`, `rt`,
`game`, `vk`) set that environment up for you ([developing](developing.md#test-a-change-against-a-base)); a variable you
set yourself (such as `RX_CPM`) wins over it.

### Windows

1. Install Python 3 from [python.org](https://www.python.org/downloads/), not the Microsoft Store build (`python -I`
   doesn't see packages installed there), then `python -m pip install -r requirements.txt`: for the development tools
   and the checks, not the build.
2. The build tools: `mise run tools` (`cargo run --release -p longshot-builder -- dev tools install . --system windows-x64`) puts `tools.lock`'s trimmed llvm-mingw
   (clang, lld and the x86-64 MinGW-w64 headers and libraries), its `cpm` (the Vulkan, SPIR-V and VMA headers, FFmpeg,
   dav1d) and `sdl` (`SDL2.dll`, `SDL3.dll`) in the checkout's `tools\`, where `longshot-builder --root .` takes them. Your
   own llvm-mingw instead: `LONGSHOT_OWN_TOOLS=1` with `LONGSHOT_MINGW`, `RX_CPM` and `LONGSHOT_SDL_DIR`; without
   `LONGSHOT_MINGW` the build takes clang from the PATH or emsdk (`C:\emsdk`, or `EMSDK`), with Visual Studio's C++
   tools and the Windows SDK (the MSVC target).
3. RenHDrum (some controllers' vibration): `python -I scripts/renhdrum.py build` makes `renhdrum.dll` from
   `third_party/renhdrum`, with a pinned Rust it downloads. The renderer's play profile is committed in
   `assets/profiles/`: nothing else to copy.

### Linux

The build needs clang and lld, the Vulkan, SPIR-V and VMA headers and FFmpeg: `mise run tools` (`cargo run --release -p longshot-builder -- dev tools install . --system
linux-x64`) puts `tools.lock`'s (the trimmed LLVM 23.1.5, the glibc 2.35 sysroot, FFmpeg with dav1d, SDL2, the
headers) in the checkout's `tools/`, as longshot-builder carries them. Without them, the PC's: clang from `LONGSHOT_LLVM`,
`EMSDK` or the PATH, the headers from `RX_CPM`, else `LONGSHOT_VULKAN_HEADERS` / `LONGSHOT_SPIRV_HEADERS` /
`LONGSHOT_VMA_HEADERS`, else `/usr/local` and `/usr`, and FFmpeg from `LONGSHOT_FFMPEG` with `lib/libavcodec.a`, else
`pkg-config`. A missing one stops the build with its code
(`compiler_missing`, `vulkan_headers_missing`, `ffmpeg_missing`; [when a build stops](reference/build.md#when-a-build-stops)).
The release's Linux `longshot-builder` is linked against glibc 2.35 (tools.lock's sysroot; its windows load X11 or
Wayland at run time) and carries the same tools, so it and a game built with it run on glibc 2.35 and newer with a Vulkan 1.3 driver (mesh shaders recommended). Nobody makes the tools themselves to
build: CI makes each archive from its pins and uploads it, and `tools.lock` names it. Only someone bumping a pin makes one
(`builder/tools/make.sh`, or the Make tools workflow: [bumping a tool](reference/releases.md#bumping-a-tool)).

`RX_TARGET=windows` builds the Windows game on Linux, with llvm-mingw's Linux build in `LONGSHOT_MINGW` (the checkout's
Windows `cpm` and `sdl` from `cargo run --release -p longshot-builder -- dev tools install . --system windows-x64`: its llvm-mingw, a Windows program, is not taken).

### Upscaler SDKs (optional)

Longshot never commits or ships vendor SDKs. `scripts/fetch.py` downloads each from its vendor at one pinned commit,
checks every file's SHA-256, and asks you to accept the vendor's license first (`--help` has every option):

| Command | Gives you |
|---|---|
| `python -I scripts/fetch.py dlss [--platform linux\|windows]` | NVIDIA's DLSS SDK, in your development folder's `sdk/ngx-<platform>`, where `dev.py`'s builds take it; for other builds, `LONGSHOT_NGX_SDK=<sdk folder>` |
| `python -I scripts/fetch.py fsr [--game <game folder>]` | AMD's FSR 4 runtime (Windows); build with `--upscaler fsr4` |
| `python -I scripts/fetch.py xess [--game <game folder>]` | Intel's `libxess.dll` (Windows) |

Each takes another folder as its last argument. A player's build needs none of this: `longshot-builder --upscaler ...
--accept-licenses ...` downloads the same pinned files itself ([the upscalers' runtimes](reference/build.md#the-upscalers-runtimes)).
`dev.py`'s builds take DLSS when they find an NGX SDK, and FSR 4 with `--fsr`.

Never share a game folder built with the DLSS SDK. On Windows, never put NGX's core (`_nvngx.dll`) beside the game.
A change to the pinned DLSS release updates `scripts/fetch.py` (`DLSS_COMMIT` and its table), `tests/test_ngx_sdk.py`
and the release named in the Longshot Launcher's license text (`App/ViewModels/SettingsStepViewModel.cs` in
MagicBeanDev/LongshotLauncher).

## 4. Build a game folder from your dump

You need your own dump of the supported version: program `0400486024DEC000`, update v131072, build `e0f3ccfa`. Other
versions don't work, and the build reads the dump where it lies without changing it. A first build needs about 5 GB
free ([the build's disk space](reference/build.md#disk-space)); build on an SSD if you can (a hard drive works, much slower, and the
build warns). The game folder reads its romfs from the .nup: keep the .nup where it is. For memory at run time, see
the [requirements](../user/requirements.md).

```sh
longshot-builder build --game "path/game.nup" --out <game folder> --language en-US    # from a .nup
longshot-builder build --game <folder with romfs/ and exefs/> --out <game folder>     # from an unpacked dump
longshot-builder check --game "path/game.nup"                                         # check the game and version only
longshot-builder update <game folder>                                                 # build the folder again
```

`mise run build -- <the same arguments>` compiles this checkout's `longshot-builder` and runs it with the tree, your
development folder's store and its `work/` as the work folder (`dev.py packs run --`). Useful options: `--work
<folder>`, `--jobs N`, `--compile-shaders most|all`, `--no-prepare-shaders`, `--upscaler dlss,fsr4,xess` with
`--accept-licenses nvidia,amd,intel`; all of them, the switches and the errors are in [the build](reference/build.md). How long a first build
takes: [build times](reference/build.md#build-times). The built game's own arguments are in [advanced
setup](../user/advanced.md#start-the-game-from-the-command-line).

Once you have one build, don't build whole game folders to try a change: build it
[against a base](developing.md#test-a-change-against-a-base).

## A release's longshot-builder

Players build through the [Longshot Launcher](https://longshot.magicbean.dev/download) only. To try a release's builder
yourself, take its file for your platform from [the releases](https://git.magicbean.dev/MagicBeanDev/Longshot/releases)
(`longshot-builder-<v>-linux-x64`, `longshot-builder-<v>-windows-x64.exe`), check it against `SHA256SUMS`, and run it as
above; it needs nothing else, and unpacks what it carries into its cache at the first use (about 2 s).
`--cache` (or `LONGSHOT_CACHE`) on an external SSD keeps that, the work folders and the stores there.

## Repository layout

| Path | What |
|---|---|
| `builder/` | The builder: `longshot-builder`, which the launcher runs, and its lifter's commands for developers, `longshot-builder dev` ([the builder](reference/builder.md), [the build](reference/build.md)); `builder/rt/`, the runtime the lifted code and the host share |
| `longshot-rs/` | The one Rust staticlib the game links (`liblongshot_rs.a`), over the game's crates as they come; built with the workspace's `game` profile (the root `Cargo.toml`) |
| `host/` | The Horizon OS runtime and the native NVN2 layer (`nvn_native.c`); `host/posix/` has the Win32 calls on Linux |
| `game/` | The game services, frame phases and hook sites: `game_call` (`game.h`), the one way host code calls the game's functions, taking a virtual core back for a thread that gave its own up; the game's flags (`flags.h`), the one reader of GameDataMgr; the frame phases (`phases.h`), host callbacks run in order on the main thread at the window acquire, once a frame in every state; and the hook sites' flags and callbacks (`sites.h`, `sites.def`, [hooks.md](reference/hooks.md)) |
| `game/sites/sites.toml` | The hook sites' definitions: what each is and its callback's contract, the same in every game build ([hooks.md](reference/hooks.md)) |
| `game/sites/` | One folder per supported game build, `<build id>/locations.toml`: where each hook site is in that build, which the lift finds and hooks ([hooks.md](reference/hooks.md)). Function names, offsets and a few masked instruction words; no other game data. In `host/` because every `longshot-builder` carries it: a pull request built with a nightly's tools gets its own tables |
| `gpu/vk/` | The Vulkan renderer, with Eden's shader recompiler and sirit ([architecture](reference/architecture.md)) |
| `gpu/render/`, `gpu/third_party/` | The op stream the host records and the renderer replays; suyu's headers, kept whole (none of its sources is compiled) |
| `scripts/` | Development builds, test runs, profile readers, release tooling (`longshot_builder.py`: the release's `longshot-builder` built with the pinned Rust; its pack and deltas are `longshot-builder dev release`); `scripts/linux/` (the Linux build tools) |
| `tests/` | The game-free checks ([developing](developing.md#run-the-checks)) |
| `third_party/` | Unmodified upstream code, each part with its `PROVENANCE.md` |
| `assets/` | Shipped data: the guest profile and the renderer's profile (`profiles/`), pipeline state lists (`pipeline_states/`), scrubbed saves (`saves/`) |
| `docs/dev/` | These pages; `docs/user/` is the player docs the website publishes |
