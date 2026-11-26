# build-core

What every stage of the builder (`longshot-builder`) shares, so the stages say the same things the same way: the
build's events, the errors a player reads, how many workers a step runs, the memory gate, safe file writes, the cache
keys and the work folder's lock. It sits under every builder crate and holds no stage of its own.

## Main parts

- [`src/report.rs`](src/report.rs): `Event`, `Reporter` (shared by every worker), `Format`: JSON Lines with `--output json`,
  text for a person otherwise; `VERSION`, the events' `"v"`.
- [`src/error.rs`](src/error.rs): `Error` (`Error::Player`: the sentence a player reads with a stable code; or a plain
  error with its cause), `IoContext`, `Result`.
- [`src/jobs.rs`](src/jobs.rs): how many workers a step may run (`--jobs`, `LONGSHOT_JOBS`, a ceiling for every step).
- [`src/memory.rs`](src/memory.rs): the memory gate: a big job starts only when the memory free holds it.
- [`src/fsx.rs`](src/fsx.rs): `write_atomic`, `write_if_changed`, copies and links safe against a stopped run, a full
  disk or another program in the folder.
- [`src/key.rs`](src/key.rs): `Key`, fed named parts in a fixed order, and `fingerprint`, the short form the log prints.
- [`src/lock.rs`](src/lock.rs): one build at a time in a work folder, and in a game folder; one game per profile
  (`ProfileLock`, play's).
- [`src/res.rs`](src/res.rs): Windows `.res` and `.ico` files, as the builder writes them.
- [`src/vulkan.rs`](src/vulkan.rs): the Vulkan loader asked for its GPUs as the game asks at its start (`doctor`'s,
  in a process of its own); with `sys.rs`, the one other module that allows `unsafe`.
- [`src/sys.rs`](src/sys.rs): what only the system can tell (the memory free, a process tree, the registry, the
  programs running), `stay_awake`, and a game's window brought to the front.

## Used by / uses

Used by `game-files`, `image`, `lift`, `build-compile`, `build-assets`, `build-driver`, `steam` and
`longshot-builder` (and `synthetic-game`'s test). Uses serde, serde_json, sha1_smol, thiserror; windows-sys on Windows.

## Rules it keeps

- **The events are a launcher's contract.** With `--output json` stdout carries the events and nothing else, each line whole
  and flushed at once, from any thread. An error's and a warning's `code` is stable; a change an older reader would
  misread raises `VERSION`. Every event and code a launcher reads is listed in
  [build-events.md](../../../docs/dev/reference/build-events.md).
- **A key holds every input that changes the output, and nothing else**, its parts in a fixed order, so two builds'
  keys differ exactly where their inputs do. SHA-1.
- A file the build writes is whole or not there (written beside its place and renamed in); a file the same as before
  is not touched.
- A lock, not a marker file: it goes with the process that held it, so a stopped build never keeps the next one out.
- `unsafe` is denied in the workspace; `sys.rs`'s Windows module is where this crate allows it, each call a plain query
  on memory the module owns.

## Tests

```
cargo test -p build-core
```

The worker count's sources and order, keys' parts kept apart and their SHA-1, atomic writes, links and copies,
a second build refused until the first lets go, the memory gate's limits and patience, JSON Lines with the version and
the overall progress, text once a percent, `.res` and `.ico` read back.

See [the events](../../../docs/dev/reference/build-events.md), [the compiler memory gate](../../../docs/dev/reference/build.md#the-compiler-memory-gate)
and [the stores and the work folder](../../../docs/dev/reference/builder.md#the-stores-and-the-work-folder).
