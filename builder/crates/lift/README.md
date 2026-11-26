# lift

The function builder and the whole lift. The function builder turns one discovered function (a `cfg::plan::Plan` entry
and its instructions from the `image::Image`) into a `mir::Function`, its edits written in as it is built. The whole
lift (`lift::whole`) is the build's lift stage: from the work folder's image and the tree's tables to every file of
gen/ the compile builds, with a key that skips an unchanged lift.

## Main parts

- [`src/lib.rs`](src/lib.rs): `Program` (the plan, the image and the edits a lift builds from; `Program::inputs`, what
  lifting a function reads, its key's input), `Lifted`, `Skip`, `single` (one word as a function).
- [`src/whole/mod.rs`](src/whole/mod.rs): `run(&LiftOptions, &Reporter)` and `run_with` (also the `Session` the
  compile makes texts from), `LiftOptions`, `LiftOutcome`, `discover`, `edits`, `Texts` (a unit's text made when the
  compile asks; the driver hands it to the compile as a `build_compile::IrText`); `KEY_FILE`, `UNITS_FILE`, `EDITS_FILE`.
- [`src/whole/units.rs`](src/whole/units.rs): units cut by estimated compile cost at stable places, hot and cold units
  of their own, written in parallel.
- [`src/whole/edits.rs`](src/whole/edits.rs): the lift's edits, planned by `edits` per function.
- [`src/whole/files.rs`](src/whole/files.rs): the other gen/ files: the function table, the boot constants, the import
  stubs, the runtime (`builder/rt`, embedded) and `rt/a64_config.h`.
- [`src/whole/host.rs`](src/whole/host.rs): what the lift reads of `host/` and `game/`: the HLE functions and the entry hooks.
- [`src/whole/store.rs`](src/whole/store.rs): the lifts' store, whole lifts hard-linked by key for other work folders,
  kept by last use (two weeks, within 1 GB), never the newest nor one in use.
- [`build.rs`](build.rs), [`key_crates.rs`](key_crates.rs): the lifter's own key (`LIFTER_SOURCES`) and `EDITS_SOURCES`.

## Used by / uses

Used by `build-driver` and `longshot-builder`. Uses `a64`, `build-core`, `cfg`, `edits`, `image`, `llvm`, `mir`, `passes`,
`profile`, `sem` and `sites`.

## Rules it keeps

- **The cache key covers every input.** `lift.key` holds the lifter's sources, the edits' data, the image, the options,
  the site tables, what of `host/` and `game/` the lift reads, `main.npdm` and the profile; each function's key holds exactly what
  lifting it reads, and each unit's its functions'. A crate `lift` depends on must be in `key_crates.rs`'s `CRATES`
  (or `FILES`), or in `NOT_IN_KEY` with the reason: a test fails otherwise.
- **Deterministic**: lifting twice gives the same bytes; a unit lifted again to the same text is not written again.
  Where a unit ends depends on the functions around it only, so one function's change moves at most the boundaries
  beside it.
- Every file in gen/ is written whole by a rename, and only when its bytes change; `lift.key` is written last; a lift
  in the store counts only with its `complete` file.
- An instruction that does not decode or has no semantics is a trap at its address: the guest stops there, the build
  goes on.

## Tests

```
cargo test -p lift
```

[`src/tests.rs`](src/tests.rs): each form of edit and each kind of branch, call and table as built, an uncovered
instruction stopping the guest, the key list against the dependencies. [`src/whole/tests.rs`](src/whole/tests.rs): the
whole lift on a small synthetic image: every file written and compiled, an unchanged lift skipped and a change
rewriting its units, a unit's text made on demand the same as its file, the store giving a lift to another folder.
The tests that compile and run need clang (or `CC`); without it they fail, unless `LONGSHOT_NO_CLANG=1`.

See [the whole lift](../../../docs/dev/reference/builder.md#the-whole-lift),
[the lift's cache](../../../docs/dev/reference/builder.md#the-lifts-cache),
[the units' texts](../../../docs/dev/reference/builder.md#the-units-texts), [units](../../../docs/dev/reference/builder.md#units)
and [the gen/ folder](../../../docs/dev/reference/builder.md#the-gen-folder).
