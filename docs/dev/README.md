# Developer docs

How to set up, build, test and ship a change to Longshot, the rules it meets, and how each part works. Player
documentation is in [../user](../user) and is published on the website. Start with [CONTRIBUTING.md](../../CONTRIBUTING.md);
agents also read [AGENTS.md](../../AGENTS.md).

| Page | Read it to |
|---|---|
| [Setup](setup.md) | take a PC from nothing to a built game: tools, the Tooling repository and plugin, the compiler, your dump |
| [Developing](developing.md) | build quickly, test against a base, use the job queue, run checks and the game, measure, profile, debug, and take a change from claim to merge |
| [Standards](standards.md) | know what a change must meet: switches, code rules, fixes and checks, measured gains, pull requests |

## Reference

How each part is implemented.

| Page | Covers |
|---|---|
| [Architecture](reference/architecture.md) | the builder, host and renderer, from dump to game folder, the caches, the frame pipeline, memory, device policy, and the rules |
| [The build](reference/build.md) | running `longshot-builder`: its commands and options, build switches, the compiler memory gate, the guest profile, build times, errors by code |
| [The builder](reference/builder.md) | how `longshot-builder` works: its crates, the lift and its mid IR, units, gen/, the compile, the stores, the runtime, its checks |
| [Shader preparation](reference/shader-preparation.md) | what a build's `--prepare-shaders` run does, its journal, the supervisor that starts it again past a shader the driver dies or hangs in |
| [Builder events](reference/build-events.md) | what `longshot-builder --output json` tells a launcher: the events, the error codes, the exit status |
| [Switches](reference/switches.md) | how the game reads switches, the options file, and the switches developers use |
| [Profiling](reference/profiling.md) | the profiling mode, frame trace, Tracy's zones, draw and performance statistics, the readers |
| [Control bridge](reference/control-bridge.md) | driving a running game from tools and tests: the protocol and `longshot_ctl.py` |
| [Pipeline state lists](reference/pipeline-states.md) | the shipped pipeline lists and the pipeline tour |
| [Releases](reference/releases.md) | versions, channels, the release workflow, making a release, signing, `tools.lock` |
| [CI](reference/ci.md) | what Forgejo Actions runs, when, and on which runners |

Features: [crash window](reference/crash-window.md) · [fullscreen monitor](reference/fullscreen.md) ·
[input](reference/input.md) · [mods](reference/mods.md) · [NFC tags](reference/nfc.md) · [profiles](reference/profiles.md) ·
[saves](reference/saves.md) · [upscalers and frame generation](reference/upscalers.md)

The game's code as Longshot changes it: [hook sites](reference/hooks.md) and the [parity gates](reference/parity-gates.md)
that check a refactor of it against `main`.
