# build-driver

The builder's driver: a player's own dump to a game folder. `build::build` runs the stages in order, each with its
cache: read (`game-files`, `image`), lift (`lift::whole`), compile (`build-compile`), finish (the game folder's
files and the assets, `build-assets`) and the shader preparation. It owns the work folder, the stores and the game
folder; `longshot-builder` is a thin command over it.

## Main parts

- [`src/build.rs`](src/build.rs): `build::build`, the stages in order; `Built`, `Places`.
- [`src/stages.rs`](src/stages.rs): `Stages`, one seam per stage (the traits `GameFiles`, `Lift`, `Compile`, `Assets`), so a test runs
  the driver with stand-ins.
- [`src/options.rs`](src/options.rs): `BuildOptions`, `GameSource`, `Upscaler`, `CompileShaders`, `GuestProfile`.
- [`src/read.rs`](src/read.rs): `ReadStage`, the read stage over `game-files` and `image`;
  [`src/hle.rs`](src/hle.rs): the symbols the image binds to the host; [`src/clashes.rs`](src/clashes.rs): a mod's
  ExeFS patch that would stop a hook site refused, naming it.
- [`src/folder.rs`](src/folder.rs): the game folder's checks, the files swapped in, `longshot_build.json` for `update`;
  [`src/verify.rs`](src/verify.rs): `longshot-builder verify`, the folder checked against the installed note, and
  repaired.
- [`src/assets.rs`](src/assets.rs): the finish's assets over `build_assets::Finish`; [`src/shaders.rs`](src/shaders.rs):
  `longshot --prepare-shaders` under the builder's progress bar.
- [`src/layout.rs`](src/layout.rs): the per-user cache, a work folder per game folder, the shared stores.
- [`src/sources.rs`](src/sources.rs): the tree and tools a release carries: the pack's one writer (`write`,
  `tree_entries`, `tool_entries`, by `longshot-builder dev release sources`) and its reader, unpacked at first use;
  [`src/tools/`](src/tools/mod.rs): the build tools and the stamps (`longshot-builder stamps`); `tools.lock` read and
  checked (`lock.rs`, its one reader), the tools' archives made and unpacked the same every time (`archive.rs`), Longshot's
  store and the release API read with curl (`store.rs`), for `longshot-builder dev tools` and `dev release`;
  [`src/runtimes.rs`](src/runtimes.rs): the upscalers' vendor runtimes, pinned by SHA-256.
- [`src/log.rs`](src/log.rs), [`src/redact.rs`](src/redact.rs): the run log, without the player's home folder and
  name; [`src/times.rs`](src/times.rs): `build_times.json`; [`src/perf.rs`](src/perf.rs): `longshot-builder perf-report`.

## Used by / uses

Used by `longshot-builder` (and `synthetic-game`'s test). Uses `build-assets`, `build-compile`, `build-core`,
`game-files`, `image`, `lift`, `sites`, serde, serde_json, sha2, ruzstd and sha1_smol.

## Rules it keeps

- **The old game stays playable**: new files go in as `<name>.new` and are swapped in only once all are complete; a
  file as the last finish put it is not copied again (the record's `files`, in `longshot_build.json`).
- **A failure leaves the next build what it needs not to fail the same way**: the renderer's objects dropped after its
  link failed, the lift's key after a lifted unit did not compile, the game's objects and the lift's key after the
  game's link failed. A tree source that did not compile keeps everything.
- One build at a time in a work folder, and none into a game folder that overlaps the dump, the work folder, the
  stores or the tools: refused before anything is touched.
- The shader preparation never fails a build, and the build's times are taken before it.
- The log is redacted by the game's own rule (`host/log_redact.c`); stdout, which a launcher reads, stays as it is.
- The work folder and everything in it are made from the game and stay on the PC.

## Tests

```
cargo test -p build-driver
```

[`tests/driver.rs`](tests/driver.rs) runs the driver end to end on synthetic inputs: a stand-in read stage writes a
small image (instructions assembled here), the real lift lifts it, a stand-in compile links nothing but files; then an
update (everything reused) and a failed link (what the next build is left); a build into the dump, or a second build
into one game folder from another work folder, is refused. The unit tests cover the folder's swaps, the layout, the
sources' pack, the tools, the runtimes, the times, the log and its redaction, and the perf report. The `redact`
example is what `tests/test_log_redact.py` compares with the C rule.

See [a build](../../../docs/dev/reference/builder.md#a-build), [what a release carries](../../../docs/dev/reference/builder.md#what-a-release-carries),
[the stores and the work folder](../../../docs/dev/reference/builder.md#the-stores-and-the-work-folder),
[build a game folder](../../../docs/dev/reference/build.md#build-a-game-folder) and
[when a build stops](../../../docs/dev/reference/build.md#when-a-build-stops).
