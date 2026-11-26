# longshot-builder

The builder's command: a player's own dump to a game folder. The Longshot Launcher runs `longshot-builder build --output json`;
a release ships this program as one self-contained file per platform, carrying the tree and the build tools it builds
with. It parses the command line and hands the work to `build-driver`.

## Main parts

- [`src/main.rs`](src/main.rs): the commands:

  ```
  longshot-builder build --game <file.nup | dump folder> --out <folder> [options]
  longshot-builder update <folder> [options]
  longshot-builder check --game <file.nup | dump folder>
  longshot-builder image | index | lift | compile ...        (developers: one stage at a time)
  longshot-builder prepare <folder>                          (a built folder's shaders prepared again, supervised)
  longshot-builder share <folder> [--out <dir>]              (the pipeline list's zip)
  longshot-builder sources | perf-report | stamps
  longshot-builder play [--game <copy>] [game arguments]     (the folder beside it updated, then the game; by the name
                                                             Longshot.exe or Longshot.x86_64 with no command: src/play.rs;
                                                             one game per profile: src/single.rs)
  longshot-builder steam-shortcut [--profile <dir>] [--name <text>] [--remove]   (a Steam tile: src/steam_cmd.rs)
  longshot-builder doctor [--profile <dir>]                  (what could stop Longshot here, with fixes: src/doctor.rs)
  longshot-builder dev discover | sites | lift | profile | site-hooks | words | decode ...   (the lifter's own)
  longshot-builder dev tools check | install | add | missing | publish | make  (tools.lock: developers, CI, the release)
  longshot-builder dev release sources | unpack | attach | dry-run | fetch-bases | deltas | files | recomplauncher   (the release's)
  ```

- No pack is compiled in: a release's program carries its pack appended after the link (`longshot-builder dev release
  sources` makes it, `dev release attach` appends it; build-driver's `sources.rs` reads it from the program's own file).
  A developer's build carries none and builds from `--root` or its checkout.
- [`src/dev/`](src/dev): `longshot-builder dev`, the lifter and its parts run alone, over a work folder's image, without
  the rest of a build (players never run it; `longshot-builder lift --work <dir>` is the lift as a build runs it). It is
  left out of the main usage; `longshot-builder dev --help` lists it:
  - [`mod.rs`](src/dev/mod.rs): the usage and `decode <hex word> ...`, each word as the decoder sees it;
  - [`discover.rs`](src/dev/discover.rs): `discover --image <dir>`: the image's discovery, its counts and time;
  - [`sites_cmd.rs`](src/dev/sites_cmd.rs): `sites --image <dir> --tables <game>`: the build's hook sites read,
    resolved and placed over the image's discovery, a line each; exit 1 naming a site that does not resolve or fit;
  - [`lift_cmd.rs`](src/dev/lift_cmd.rs): `lift`: the whole lift, from the image to gen/ (`lift::whole`), with its
    switches: `--profile-generate`, `--profile`, the IR's (`--no-polls`, `--no-mark-bitmap`, `--sync-all-at-calls`,
    `--mark-each`, `--no-simplify`), the units' costs, `--jobs`, `--store`, `--force`, `--output json`. It
    writes the units' texts;
  - [`profile_cmd.rs`](src/dev/profile_cmd.rs): `profile --map <profile_map.txt> --raw <counts> ... [--into <profile>]
    --out <profile>`: a run's raw counts made a guest profile;
  - [`site_hooks.rs`](src/dev/site_hooks.rs): `site-hooks --tables <game> --build <build id> --out <file.ll>
    <name>=<site>...`: hook sites as the lift lowers them, each a function a C test calls, for the site tests
    (`tests/sitehooks.py`; docs/dev/reference/builder.md, "Building and checking the builder");
  - [`words.rs`](src/dev/words.rs): `words --in <file> --pc <hex> --out <file.ll> --list <file> [--count-checks]`:
    each word as a function of its own, for `tests/test_lifter_semantics.py`;
  - [`tools_cmd.rs`](src/dev/tools_cmd.rs): `tools`: `tools.lock`, the build tools the builder carries (over build-driver's
    `tools::{lock, archive, store}`): `check [--require]`, `install <folder> --system <s>`, `add`, `missing`, `publish`,
    `make <folder> --out <archive>` (docs/dev/reference/releases.md, "tools.lock");
  - [`release_cmd.rs`](src/dev/release_cmd.rs): `release`, a release's packaging, run in CI by a builder built first:
    `sources` (the pack it carries, by build-driver's `sources` writer: the same files give the same bytes), `unpack`
    (a pack into a plain folder, checked), `dry-run <program>` (a player's build's dry run, with and without AMD FSR 4,
    checked), `recomplauncher <full file>` (RecompLauncher's zip: the program as `Longshot.exe`, made a windowed
    program, or `Longshot.x86_64`, stored, the same files giving the same bytes);
  - [`release_deltas.rs`](src/dev/release_deltas.rs): `fetch-bases` (the channel's last 5 releases' full files, from
    the release API by curl, each checked against its manifest), `deltas` (`zstd --patch-from` from each, checked to
    give the full file back, and the manifest), `files` (a release's stamps, bases, deltas and manifest together).

## Used by / uses

Used by nothing in the workspace (it is the program); `tests/test_lifter_semantics.py` runs `dev words`, the site tests
(`tests/sitehooks.py`) `dev site-hooks`; CI's release jobs run `dev tools` and `dev release`. Uses `build-driver`,
`build-core`, `build-assets` (`share`), `steam` (`steam-shortcut`) and `game-files` (`doctor`'s disk); `dev` uses `a64`, `cfg`, `edits`, `image`, `lift`, `llvm`, `mir`, `passes`,
`profile` and `sites`, and `dev release` serde_json (the manifest) and crc32fast (the zip).

## Rules it keeps

- **Its output is a launcher's contract**: with `--output json`, stdout carries the versioned JSON Lines events and nothing
  else ([build-events.md](../../../docs/dev/reference/build-events.md)).
- **Exit status**: 0 done (`check`: the supported build), 1 stopped (the `error` event says why; `check`: another
  build), 2 a command line it does not take.
- A bug's panic, on any thread, is said as an `error` event and the process ends with status 1.
- No player's build runs Python.
- `dev` runs the same code a build does (`lift::whole`, `cfg`, `sites`), so what it finds is what a build finds. Its
  inputs and outputs (an image, gen/, a profile's raw counts) are made from the game: they stay on the PC.

## Tests

```
cargo test -p longshot-builder
```

[`tests/cli.rs`](tests/cli.rs) checks the command line's own answers (`--help` is the usage with status 0, `--version`
says what the program carries). [`tests/panic.rs`](tests/panic.rs) runs the program's `panic-test` command (a debug
build's, which panics on a worker thread) and checks the `error` event and status 1. The stages are tested in their
crates, and the build that `longshot-builder build` runs (`build_driver::build::build`) end to end in `synthetic-game`.

See [the build](../../../docs/dev/reference/build.md) (how to run it, its switches and its errors),
[one stage at a time](../../../docs/dev/reference/build.md#one-stage-at-a-time),
[what a release carries](../../../docs/dev/reference/builder.md#what-a-release-carries) and
[the events](../../../docs/dev/reference/build-events.md#the-events).
