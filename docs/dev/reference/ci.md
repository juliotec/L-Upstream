# CI

CI is moving from Forgejo Actions (`.forgejo/workflows/`) to CircleCI (`.circleci/`). Until the cutover both run:
CircleCI runs the checks, and Forgejo Actions still publishes the releases and deploys the website. Neither needs the
game: every check is game-free.

**Run CI's checks yourself:** `mise run ci` runs what CI checks on every change: the pinned test tools (`test-deps`),
every game-free check (`tests/run_all.py --strict`, the same line CI runs in parts) and the website's build and checks
(`scripts/website_check.sh`). `mise run test` runs the checks alone, and takes `run_all.py`'s arguments.

## CircleCI

CircleCI uses the team's orb, `magicbeandev/ci` (alias `mb`). `.circleci/config.yml` has one `mb/setup` job per event;
it compares the changed files with `.circleci/mapping.conf` and continues with that event's workflow:

| Event | Workflow | What runs |
|---|---|---|
| Pull request (same repository) | `events/pr.yml` | The checks the changed paths call for, a website preview, then `gate` |
| Push to `main` | `events/main.yml` | Every check, then the nightly release (dry run until the cutover) and the website deploy (dry run) |
| Tag `v*` | `events/release.yml` | Every check, the Windows ones included, before anything is built, signed or published (dry run) |
| Schedule `nightly` | `events/nightly.yml` | `third-party-upstream`: the `third_party/` parts against their upstream archives |
| Schedule `website_refresh` | `events/scheduled.yml` | The website deploy (dry run) |
| API run | `events/manual.yml` | A release dry run, or the Windows Vulkan back end linked with DLSS |
| Fork pull request | `events/fork.yml` | The pull request checks, with no secrets (below) |

Every workflow ends at `mb/gate`, which posts the `ci/circleci: gate` status on the commit.

The checks, in `.circleci/shared.yml`:

| Job | What it checks |
|---|---|
| `lint` | pre-commit on the changed files, and actionlint (`mise run lint`) |
| `third-party` | `scripts/check_third_party.py` |
| `guide-screenshots` | the player guide's screenshots and their source review |
| `test-linux` | `tests/run_all.py`, in 4 parts on x86-64 |
| `builder-cli` | the Windows builder's `--cli` program |
| `spirv-fixtures` | the shaders' SPIR-V fixtures |
| `linux-appimage` | the Linux builder's AppImage |
| `windows-builder` | the Windows builder's zip, its PE files and a start under Wine |
| `windows-vulkan` | the Windows Vulkan back end, cross-compiled with llvm-mingw |
| `website` | the website's build and checks |

`mapping.conf` turns on areas by path (`tests`, `gpu`, `linux`, `windows`, `website`, or `full` for everything);
`lint`, `third-party`, `guide-screenshots` and `website` (the site's checks) always run, and a docs-only change runs
nothing else; `website` turns on the site's preview (pull requests) and deploy (`main`). `main`, tags and API
runs run everything.

**Fork pull requests.** CircleCI's Forgejo integration skips them, so a second CircleCI project (`Longshot-forks`)
runs `.circleci/forks.yml` from a Forgejo webhook. A maintainer adds the label `ci: approved` after reading the change;
the doorbell job checks the pull request (open, from a fork, labelled, its head commit) and runs `events/fork.yml` on
that commit with no secrets. Every push runs again while the label is on; remove it to stop, add it again to rerun.
After 14 days with no push the label is removed.

**Contexts** (secrets): `release-signing`, `forgejo-bot`, `cloudflare-prod`, `forgejo-comment`.

**Checking a change to the config:** `MB_ORB_DIR=<a checkout of MagicBeanDev/orb> pre-commit run --hook-stage manual circleci-config-validate`.

## Forgejo Actions

| Workflow | When | What it does |
|---|---|---|
| `ci.yml` | Pull request, push to `main` | The pull request's title, actionlint, the website's checks (`scripts/website_check.sh`) and `tests/run_all.py` in two parts |
| `pr-description.yml` | Pull request | The description against the template (`scripts/check_pr_description.py`) |
| `third-party.yml` | Pull request, push, nightly | `scripts/check_third_party.py`, and the upstream comparison at night |
| `guide-screenshots.yml` | Pull request, push | The guide screenshots' source review |
| `release.yml` | Push to `main`, tag `v*` | The nightly or the stable release: calls `build-linux.yml` and `build-windows.yml`, signs and publishes ([releases](releases.md)) |
| `website.yml` | Push to `main` that changes the site's sources, hourly, pull request | Deploys longshot.magicbean.dev; a pull request gets a preview |
| `vulkan-dlss.yml` | By hand | The Windows Vulkan back end built with NVIDIA's DLSS SDK and AMD FSR 4 |
| `vulkan-fixtures.yml` | By hand | The authored Vulkan fixtures on Mesa's software driver |

### Runners

Every job runs on Linux; there is no Windows runner. Windows files are cross-compiled with llvm-mingw's Linux build and
started under Wine. The labels the workflows ask for, all on the image `docker://ghcr.io/catthehacker/ubuntu:act-24.04`:

| Label | For |
|---|---|
| `ubuntu-latest`, `ubuntu-24.04` | the checks |
| `quick` | jobs of a few minutes (title, description, third-party, a release's version and publish), on a runner kept for them with capacity 1 so they never wait behind a build |
| `release` | the builders' builds, on a runner that carries no other label, so CI never takes it. It needs about 50 GB of disk |

Register a runner with `forgejo-runner register --instance https://git.magicbean.dev --token <token> --labels ...` from
MagicBeanDev's *Settings > Actions > Runners*, and run `forgejo-runner daemon`. git.magicbean.dev is behind Cloudflare,
which cuts uploads at 100 MB: the organization variable `RELEASE_UPLOAD_API` gives a release runner the server's
direct address ([one-time setup](releases.md#one-time-setup)).

## Before you push

Run `mise run lint` and `mise run test` and say so in the pull request. CircleCI's runs, logs and reruns are visible to
members of the MagicBeanDev organization on CircleCI ([CONTRIBUTING.md](../../../CONTRIBUTING.md#ci)).
