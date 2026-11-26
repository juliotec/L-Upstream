# CI

CI is Forgejo Actions (`.forgejo/workflows/`): it runs the checks, publishes the releases and deploys the website. No
check needs the game: every one is game-free.

**Run CI's checks yourself:** `mise run ci` runs what CI checks on every change: the pinned test tools (`test-deps`),
every game-free check (`tests/run_all.py --strict`, the same line CI runs in parts) and the website's build and checks
(`scripts/website_check.sh`). `mise run test` runs the checks alone, and takes `run_all.py`'s arguments; `mise run lint`
runs pre-commit and actionlint as the `lint` job does. `tests/run_all.py --container` runs the checks in the CI
image itself ([below](#the-ci-images)).

## Forgejo Actions

| Workflow | When | What it does |
|---|---|---|
| `ci.yml` | Pull request, push to `main` | `changes`: what the rest needs (the [CI images](#the-ci-images), and whether the compiling jobs run); `lint`: pre-commit on the changed files, ShellCheck on `scripts/` and actionlint; `third-party`: `scripts/check_third_party.py` and the guide screenshots' source review; `rust`: rustfmt, clippy, `cargo test` (with clang, lld and llvm-mc), then the synthetic game end to end from the same build; `host-windows`: the host's C for Windows with llvm-mingw; `renderer`: the renderer built alone (`longshot-builder compile --only renderer`, a developer's builder from cargo, tools.lock's compilers), for Linux and for Windows, each without and with AMD FSR 4; `tests`: `tests/run_all.py --strict` in 2 parts (1200 s a check; every part builds the developer `longshot-builder` first, for the site-hook checks, and runs the checks that declare `RUN_ALL_ALONE` (they measure time or CPU) one at a time after the others; the part whose checks run the release `longshot-builder` or `tests/test_lifter.py` builds those first; the shader recompiler's SPIR-V fixtures are three of its checks); `website`: the website's checks (`scripts/website_check.sh`), its `dist/` kept for the preview. On a pull request, `changes` skips `rust` and `host-windows` when [only docs change](#what-a-pull-request-runs), and `renderer` when no renderer file does |
| `pr-description.yml` | Pull request (opened, edited, pushed) | The title (a Conventional Commit) and the description against the template (`scripts/check_pr_description.py`), with main's script: no code of the pull request's runs. The title is here, not in `ci.yml`, because a title edit runs this file, and an edit's run of `ci.yml` would cancel the code's checks |
| `third-party.yml` | Nightly, by hand | The third-party parts compared with their upstream archives (`check_third_party.py --upstream`); the per-change check is `ci.yml`'s `third-party` |
| `release.yml` | Push to `main`, tag `v*` | The nightly or the stable release: names the version, calls `build-linux.yml` and `build-windows.yml` (each checks every tools.lock line's archive is in the store before it uploads anything; a release uploads no tool), signs and publishes ([releases](releases.md)) |
| `tools.yml` | By hand (any branch; uploads on `main` only) | Make tools: the build tools named made twice from their pins (`builder/tools/make.sh --check`), uploaded to the store and given as `tools.lock` lines ([bumping a tool](releases.md#bumping-a-tool)) |
| `build-linux.yml`, `build-windows.yml` | Called by `release.yml`, Windows after Linux | `longshot-builder-<v>-<p>` per platform, its deltas and manifest: each job compiles the program it ships once (`scripts/longshot_builder.py build`) and appends its pack to it (`dev release attach`); the Linux program before its pack is both jobs' tool (`dev tools check`, `dev release`: `sources`, `attach`, `dry-run`, `files`), the Windows job taking it as an artifact of the run (`longshot-builder-linux-tool`). The back end's build checks, which ship nothing, are `ci.yml`'s `renderer` job, not a release's |
| `website.yml` | Push to `main` that changes the site's sources, hourly, pull request | Deploys longshot.magicbean.dev (hourly too, for the progress data the build collects); a pull request that changes the site's sources gets a preview: `ci.yml`'s `website` job builds it once and keeps `dist/` as `website-dist-<head commit>`, and main's code uploads those files ([scripts/ci_website_dist.py](../../../scripts/ci_website_dist.py)) |
| `ci-image.yml` | Push to `main` that changes the CI images' inputs, by hand on `main` | Builds [the CI images](#the-ci-images) and pushes them to the forge's registry |
| `vulkan-dlss.yml` | By hand | The Windows Vulkan back end built with NVIDIA's DLSS SDK and AMD FSR 4 |
| `vulkan-fixtures.yml` | By hand | The authored Vulkan fixtures on Mesa's software driver |

### Runners

Every job runs on Linux; there is no Windows runner. Windows files are cross-compiled with llvm-mingw's Linux build and
started under Wine. The labels the workflows ask for, all on the image `docker://ghcr.io/catthehacker/ubuntu:act-24.04`:

| Label | For |
|---|---|
| `ubuntu-latest`, `ubuntu-24.04` | the checks. No secrets: a runner anyone in the team lends can carry these |
| `quick` | jobs of a few minutes (`changes`, `lint`, `third-party`, the title and description, the website preview's wait), on a runner kept for them so they never wait behind a build |
| `release` | the release's `longshot-builder` builds, the CI images' build, and the Make tools workflow's build of `main`'s tools (another branch's runs on `ubuntu-24.04`). **Only the server's release runner, `unraid-linux-1`**, which carries no other label, so CI never takes it; never anyone else's runner. Each such job starts with [scripts/ci_release_runner.sh](../../../scripts/ci_release_runner.sh), which fails unless the job reaches the seal broker with its pinned key, so a runner given the label anywhere else builds nothing. A pull request's own code never runs on it. It needs about 50 GB of disk |
| `deploy` | the jobs that sign, publish or deploy: a release's version and publish, the Make tools workflow's upload, the website's deploy and preview upload, the CI images' push. Only the server's quick runner, on the seal broker's network ([below](#secrets-only-our-runners-can-open)). A pull request's own code never runs on it |

The server's runners share 2 cores (MagicBeanDev/Infra, `runner-dind`): a release's jobs run there one at a time.

Register a runner with `forgejo-runner register --instance https://git.magicbean.dev --token <token> --labels ...` from
MagicBeanDev's *Settings > Actions > Runners*, and run `forgejo-runner daemon`. A runner lent by someone else gets
`ubuntu-latest` and `ubuntu-24.04` only. git.magicbean.dev is behind Cloudflare, which cuts uploads at 100 MB: the
organization variable `RELEASE_UPLOAD_API` gives a release runner the server's direct address ([one-time
setup](releases.md#one-time-setup)).

### Caches

Each runner keeps its own cache (forgejo-runner's, on its disk): `ci.yml`'s jobs read it on every run and write it only
on a push to `main`. forgejo-runner 13 keeps a pull request's writes to that pull request (its write isolation key), so a
pull request can't put anything there that another run reads. The jobs that sign or publish a release, and the
website's deploy, read no cache at all; the jobs that build a release read only [their own](#the-releases-cache).

| Cache (key) | What it holds | Who reads it |
|---|---|---|
| `git-base` | main's tree, a bare repository at depth 1 ([scripts/ci_git_base.sh](../../../scripts/ci_git_base.sh)) | `rust`, `host-windows`, `renderer`, `tests`, `website`: before `actions/checkout`, the workspace borrows its objects, so the fetch takes only what the commit adds (a lent runner fetched the whole tree, 26 MB, through Cloudflare in 2 to 4.5 minutes: now seconds). After the checkout every object is copied in and checked against its name (`git fsck`) |
| `rust-deps-<Rust>-<Cargo.lock>` | the registry crates (`/opt/cargo/registry`, the image's `CARGO_HOME`), downloaded and built, without the workspace's own: a checkout's new file times make cargo build those again whatever a cache holds ([scripts/ci_cargo_prune.py](../../../scripts/ci_cargo_prune.py)). One entry per lock, written when missing. When the lock has changed, the newest entry for the same Rust is restored instead, and cargo builds only the crates that changed | `rust`; `host-windows` and `renderer` read it |
| `<llvm-mingw>-archive`, `windows-tools-downloads-<tools.lock>` | the downloads, each checked by its size and SHA-256 when taken, written when missing | `host-windows`; `renderer` (Windows) reads `<llvm-mingw>-archive` |
| `renderer-tools-downloads-<system>-<tools.lock>` | tools.lock's archives the renderer compiles with (linux-x64's llvm, sysroot and CPM cache; windows-x64's CPM cache), each checked by its size and SHA-256 when taken, written when missing | `renderer` |
| `renderer-store-<system>-<renderer files>` | the renderer's compiled objects (`compile --store`), each by its key: its compiler, flags, source and headers (build-compile's store.rs), so an object of another tree is the one this compile would make. Written by a push to `main` whose renderer files have no entry yet, from an empty store, so an entry holds one tree's objects; a pull request restores the newest | `renderer` |
| `ccache-tests-part<K>of<N>-...` | ccache's objects, keyed by the compilers' versions and the shader recompiler's sources; the nearest older key is restored | `tests` (one per part) |
| `ccache-host-windows-<llvm-mingw>-<host/, game/, tools.lock>` | ccache's results of the host's `-fsyntax-only` compiles for Windows (`check_host_compile.py --launcher ccache`; `CCACHE_BASEDIR` the checkout and the work folder in it, the compiler checked by llvm-mingw's name, so the commands are the same every run: all 66 units hit on a second run, 12 s down to 2 s); the nearest older key is restored | `host-windows` |

The tools themselves are no cache: they are [the CI images](#the-ci-images). `ci.yml`'s jobs run with `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_DEV_DEBUG=0`: incremental state is only ever
written in CI, and the dev builds link faster without debug info (a failing test still names its file and line).

### The CI images

Every `ci.yml` job that needs more than Python runs in one of two images (`container: image:`), so no job installs
anything:

| Image | Jobs | Parts (layers) | Size |
|---|---|---|---|
| `git.magicbean.dev/magicbeandev/longshot-ci-checks:<tag>` | `rust`, `host-windows`, `renderer`, `tests` | `system` (g++, clang, lld, llvm-mc, the sanitizer runtimes, spirv-tools, ccache, cmake, libdav1d: 943 MB), `python` (the tests' packages, pre-commit: 181 MB), `headers` (glslang 16.6.0, the SPIR-V and Vulkan headers: 71 MB), `node` (138 MB), `tools` (actionlint, ShellCheck, gitleaks: 37 MB), `rust` (mise.toml's Rust, minimal, rustfmt and clippy: 667 MB) | 2.1 GB |
| `git.magicbean.dev/magicbeandev/longshot-ci-lite:<tag>` | `lint`, `website` | `system-lite` (git, curl, Python: 128 MB), `python`, `node`, `tools`, `hooks` (pre-commit's hooks fetched: 63 MB) | 0.6 GB |

(Sizes unpacked, on the 78 MB Ubuntu base. `node` is in both: the actions run in it.)
[scripts/linux/ci_setup.sh](../../../scripts/linux/ci_setup.sh) installs every part, the one list, each removing its
own caches (apt's lists, docs and translations, pip's); [scripts/linux/checks.Dockerfile](../../../scripts/linux/checks.Dockerfile)
has a stage per image and a `RUN` per part on its pinned Ubuntu 24.04, and `tests/run_all.py --container` builds
`checks` locally (`scripts/linux/checks.sh`). Every download is pinned: the distribution's signed lists for apt,
mise.lock's files and SHA-256 for Node, actionlint, ShellCheck and gitleaks, [scripts/ci_rust.sh](../../../scripts/ci_rust.sh)'s
pinned rustup for Rust. gitleaks is mise.toml's prebuilt release, run by pre-commit's `gitleaks-system` hook: the
`golang` hook built it with a Go toolchain it downloaded, 300 MB.

- **A tag is its inputs' hash** ([scripts/linux/ci_image.sh](../../../scripts/linux/ci_image.sh) `tag NAME`: the
  image's name, the Dockerfile, `ci_setup.sh`, `ci_image.sh`, `ci_rust.sh`, `fetch_commit.sh`, `mise.toml`,
  `mise.lock`, `.pre-commit-config.yaml`). `ci.yml`'s `changes` job hashes the commit's inputs and asks the registry
  for both tags, anonymously (the organization is public: no credentials are needed to pull).
- **A change to the inputs checks itself without pushing.** Its tags are not in the registry yet, so its jobs run on
  the runners' own image (`ghcr.io/catthehacker/ubuntu:act-24.04`, Ubuntu 24.04 with Node) and run their image's
  parts first (`ci_setup.sh --github-env --image <name>`): the pull request's own tools, a few minutes more a job. A
  push to `main` of such a change does the same until the images are pushed.
- **Only main pushes** (`ci-image.yml`, on a push to `main` that changes the inputs, or by hand on `main`). The
  runners give a job no Docker, so it is not `docker build`: `build` runs the image's parts in a job container of the
  Dockerfile's base (the same digest) on the server's release runner and keeps each part's folders as a layer of its
  own, made the same way every time (sorted, no times, no owners), so a part whose files didn't change is the same
  layer and a runner pulls only the ones that did; `push`, on `deploy`, stacks them on that base with crane
  (go-containerregistry, pinned by SHA-256), sets the images' environment (`ci_setup.sh env`), and pushes with
  `PACKAGES_TOKEN_SEALED` (`write:package`), which only main's runs can open
  ([below](#secrets-only-our-runners-can-open)), through `RELEASE_UPLOAD_API`'s host (Cloudflare cuts uploads at
  100 MB). A pull request never runs `ci-image.yml` and gets no such token, and a tag in the registry is never pushed
  again: a pull request can't replace the images main's runs use.

### The release's cache

`build-linux.yml` and `build-windows.yml` keep `release-downloads-<platform>-<what pins them>`: the downloads only,
never anything unpacked or compiled. That is the pinned Rust's archives (`$RUNNER_TEMP/rust/*.tar.xz`: the Linux host's
rustc and cargo, and each target's standard library, Windows' too on Windows), the `.crate` files of `Cargo.lock` and
`third_party/renhdrum/Cargo.lock` (`rust/crates`), tools.lock's archives (`tools-cache`; on Linux also
`link-tools/.cache`, the llvm and sysroot that link the builder) and, on Windows, llvm-mingw's archive (`llvm/.cache`).
The key hashes every file that pins them: both `Cargo.lock`s, `scripts/renhdrum.py` (the Rust), `tools.lock`, and on
Windows `builder/tools/pins.sh` (llvm-mingw). It is restored and saved only by a run of `main` or a `v*` tag, on the
server's release runner, and saved right after the job's last download (the Rust builds), not at its end, so a later
step that fails still leaves the next run its downloads: otherwise that run fetches the Rust and every crate again, one
at a time (about 4 minutes). A key change restores the newest entry for the platform.

They keep one more, `release-rust-target-<platform>-dist-<target>-<renhdrum.py>-<Cargo.lock>`: cargo's target folder
of the shipped builder's one compile (`$RUNNER_TEMP/rust-work/target`), restored right before it and saved right after
it, by the same runs only. Another Cargo.lock restores the newest entry for the same platform, profile, target and
pinned Rust (`scripts/renhdrum.py` pins it); another Rust starts cold. Cargo compiles again whatever its fingerprints
find changed, which is every crate of the checkout's own (a fresh checkout's files are all new) and any crate whose
version or flags changed; the third-party crates' compiled code is reused. A cold compile of the builder takes about
17 minutes on the release runner's 2 cores. Warm, only the workspace's crates and the link are left; until the job
logs show it, expect roughly half that.

**This is a weaker guarantee than the downloads', and it was chosen for the time.** What the target folder holds is
not checked again: an entry someone else wrote would be linked into the program the release signs. What protects it:

- Only a run of `main` or a `v*` tag restores or saves it (each step's `if:`), on the server's release runner, whose
  cache server only that runner's jobs reach (forgejo-runner's cache is per runner and per repository).
- The jobs that run on the `release` runner are `release.yml`'s builds (pushes to `main`, `v*` tags) and `tools.yml`'s
  build of `main`; another branch's `tools.yml` runs on `ubuntu-24.04`. A pull request's own code never runs there,
  and forgejo-runner keeps a pull request's cache writes to that pull request; no pull request's job uses a
  `release-` key.
- Left open: someone who can push a branch to this repository could write a workflow that runs on `release` and saves
  a key of this name, since the label only routes a job and the `if:` is in the workflow they would replace. Pushing
  branches is the same trust as pushing to `main` here. A poisoned entry would reach a signed build; a release cut
  from a cold cache (delete the `release-rust-target-` entries) does not depend on it.

Why the downloads keep the guarantee that what a release ships comes from nothing a cache holds:

- **Every file is checked by its SHA-256 each time it is used**, against the pins in the release's own checkout:
  `scripts/renhdrum.py`'s `fetch()` and `vendor_crates()` hash each archive and crate before unpacking it, and download
  it again when it differs. `scripts/longshot_builder.py llvm-mingw` does the same for its archive, and
  `longshot-builder dev tools install` checks each tool by its size and SHA-256. A wrong file costs a download, never a
  different build.
- **Nothing unpacked is kept.** The Rust toolchain is unpacked again from the checked archives every run
  (renhdrum.py trusts an unpacked toolchain without checking, so it is never cached). Every object and file that
  publish signs is made in that run, from that checkout, but the builder's compiled Rust (above) and the third-party
  objects a published, signed release already had ([releases](releases.md#the-release-workflow)).
- **Who can write a `release-` key:** forgejo-runner's cache is per runner and per repository, and keeps a pull
  request's writes to that pull request. A run of another branch on the release runner could still save a key of this
  name, since the label only routes a job. That is why nothing rests on who wrote the cache: the hash checks above.

### What a pull request runs

`ci.yml`'s `changes` job reads the pull request's changed files from the API ([scripts/ci_changes.py](../../../scripts/ci_changes.py))
and skips `rust` and `host-windows` when every one of them is in a place those jobs never read: `website/`,
`docs/` (but `docs/dev/reference/build-events.md`, which the builder's tests compare with its codes), the
other workflows' files and the top-level documents. The Rust workspace's tests and build scripts read `host/`, `gpu/`,
`game/`, `assets/`, `third_party/`, `scripts/`, `tests/` and `tools.lock`, so any other change runs them all. Anything
unsure runs them too: a push to `main`, a run by hand, an API that doesn't answer. `renderer` runs only when a changed
file is one `compile --only renderer` reads, or what builds it (`ci_changes.py`'s `RENDERER`: `gpu/`, the host and game
headers and the host files the renderer compiles, its third-party parts, its logo and profile, `builder/`, the Rust
pins, `tools.lock`, the CI image's tools, and its job's own files); anything unsure runs it too. `lint`, `third-party`, `tests` and
the website's checks run on every pull request (the checks and the site read files from all over the repository).

A skipped job is not a failed one: `main` requires no status checks, and Tooling's `forge.py pr status` counts
`skipped` as passed. A pull request's newer push cancels its older runs of `ci.yml` and `pr-description.yml` (one run per pull request at a time, the newest); `release.yml` is never
cancelled, and runs of `main` aren't either.

### The website's preview

A pull request that changes the site's sources (`website/`, `docs/`, `CONTRIBUTING.md`, `website.yml`) gets a preview
of the built site, a version of the Worker with the alias `pr-<number>`. The site is built once, by `ci.yml`'s
`website` job (`pull_request`, no secrets), which keeps `dist/` as the artifact `website-dist-<head commit>`.
`website.yml` runs from `main` (`pull_request_target`), for a pull request from a branch of Longshot on every push, or
for a fork's when a maintainer adds the label `preview` after reading it, in two jobs:

- `preview-wait` (`quick`, no secrets): main's [scripts/ci_website_dist.py](../../../scripts/ci_website_dist.py)
  waits, up to 30 minutes and saying so each minute, for that artifact. Only one of `ci.yml`'s `pull_request` run at
  the pull request's head commit counts. If the website job failed or was cancelled, there is no preview.
- `preview-upload` (`deploy`, `enable-openid-connect`): checks out main, takes the artifact as files only (a link, an
  absolute path or `..` is refused), and uploads it with main's wrangler and `wrangler.jsonc` and the sealed Cloudflare
  token; then it posts the link on the pull request and takes the label off. It never runs the pull request's code:
  nothing of the pull request is checked out or executed there, so the token is opened only beside main's code.

### Secrets only our runners can open

A label only routes a job: a runner says its own labels each time it starts, so anyone's runner can ask for `deploy`
or `release`, and Forgejo hands it the job and the job's secrets. So the secrets that sign or deploy are kept in
Forgejo **sealed** ([scripts/ci_secret.sh](../../../scripts/ci_secret.sh), AES-256 with PBKDF2), and the key that
opens them, `seal.key`, is on the server only, in the **seal broker** (MagicBeanDev/Infra's README, *The seal
broker*). The broker hands it to a job of **main or a `v*` tag** and to no other, and it knows which run a job is from
by the ID token Forgejo signs for the job, not by anything the workflow says: a workflow on another branch, or a pull
request's copy of one, sets its own env, steps and container, and a label only routes it. A job that unseals:

- has `enable-openid-connect: true` (that job only: any step of it can ask for the token) and
  `SEAL_BROKER_PIN: ${{ vars.SEAL_BROKER_PIN }}` (the broker's pinned key, an organization variable);
- runs `scripts/ci_secret.sh unseal`, which fetches the key from the broker the first time (to `$RUNNER_TEMP`);
- runs only code of main or the tag: a job that checks out a pull request must not turn on `enable-openid-connect`.
  The website's preview is for that reason built by `ci.yml` (`pull_request`, no secrets), and main's code takes its
  `dist/` as files and uploads it from `deploy` (`website.yml`).

The broker gives the key to `push` on main or a `v*` tag, `schedule` and `workflow_dispatch` on main, and
`pull_request_target` (main's own workflow file) for Longshot only; anything else gets a refusal that says why. This
rests on main and the `v*` tags being protected: whoever may push to main or push a `v*` tag may have the key.

- A runner that takes such a job and can't reach the broker (anyone else's) gets text it can't open: the job fails, and
  signs or deploys nothing.
- A job of a branch, or of a pull request, gets no key, on any runner.

| Secret (organization) | Sealed | Used by |
|---|---|---|
| `RELEASE_SIGNING_KEY_SEALED` | the release key's private PEM, `release.pem` | `release.yml` publish ([signing](releases.md#signing)) |
| `PACKAGES_TOKEN_SEALED` | a Forgejo token with `write:package` alone | `tools.yml` publish: the build tools' archives uploaded as BeanTools ([releases](releases.md#one-time-setup)); `ci-image.yml` push: [the CI images](#the-ci-images) |
| `CLOUDFLARE_API_TOKEN_SEALED` | the Cloudflare token | `website.yml`, deploy and preview |
| `PREVIEW_COMMENT_TOKEN_SEALED` | optional, a Forgejo token with `write:issue` | `website.yml`, the preview's comment on a fork's PR |

`CLOUDFLARE_ACCOUNT_ID` is not sealed: it opens nothing. To seal a secret, on a PC you trust with a copy of `seal.key`:
`scripts/ci_secret.sh seal --key seal.key < release.pem`, and paste the line into the Forgejo secret. A new
`seal.key` (`scripts/ci_secret.sh newkey`) means sealing every secret again.

What sealing doesn't cover:

- A pull request's or a branch's run may still ask for `deploy` and run there: it gets no key, but it shares the runner
  (and its Docker) with the real deploy jobs. A fork's run waits for a maintainer's approval: read its `.forgejo/`
  changes before you approve one, and don't mark a fork's author as always trusted.
- A job's own token (`forgejo.token`) comes with every job, whichever runner takes it, and on a push it may write to
  the repository (releases, tags).
- A runner that takes `release` builds the files publish signs.

What the `deploy` jobs run is kept to what they need, since every step of such a job can read what it mounts:

- `release.yml`'s and `tools.yml`'s `publish` compile no Rust and run no program the checkout builds (a build script or a proc
  macro would run where `seal.key` is). The build jobs make every file publish signs, RecompLauncher's zips too;
  `tools.yml`'s uploads with `scripts/ci_tools_publish.py`, Python's standard library, the archives its make job made
  twice on a `release` runner, refusing one whose version `tools.lock` names with other bytes. The version job compiles
  nothing: tools.lock is checked by the build jobs, with the builder they build anyway.
- A secret, sealed or the job's token, is given in the `env:` of the steps that use it, never the job's.
- The jobs that make a release's files (`build-linux.yml`, `build-windows.yml`) restore only [the release's
  cache](#the-releases-cache): downloads each checked by its SHA-256 when used, and, for trusted runs only, the
  builder's compiled Rust, which is not checked again (the trade-off is written there). They run no build check that ships nothing: the back end's is `ci.yml`'s `renderer`, on the pull request runners.

Lend runners only to people you'd trust that far.

## Before you push

Run `mise run lint` and `mise run test` and say so in the pull request. CI's runs and logs are in the repository's
*Actions* tab ([CONTRIBUTING.md](../../../CONTRIBUTING.md#ci)).
