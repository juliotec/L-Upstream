# synthetic-game

The builder end to end on a synthetic game: a small AArch64 program of its own, packed as a .nup, built through every
stage of a player's build, run, then built again after a one-instruction change, which must compile that function's
unit alone. It stands in for the game, which no test may read. It is a test crate (`publish = false`); nothing depends
on it.

## Main parts

- [`game/main.S`](game/main.S), [`game/module.ld`](game/module.ld): the program (integer, floating-point and memory
  work, indirect calls, an SVC, two RomFS files read through a host import, a function big enough to be a unit of its
  own) and its module's layout, assembled and linked with the pinned clang at test time.
- [`game/host/`](game/host): the test's host, linked with the real runtime (`builder/rt`) and the real `host/romfs.c` and
  `host/romfs_index.c`.
- [`src/lib.rs`](src/lib.rs): `nso_from_elf` (the linked module as an NSO), `npdm`, `romfs_files`, `expected_result`
  (what `nnMain` returns, computed here), `BUILD_ID` and `TITLE_ID` (its own, not the game's).
- [`tests/end_to_end.rs`](tests/end_to_end.rs): the build, the run and the rebuild.

## Used by / uses

Used by nothing. Its tests use `build-driver`, `build-compile`, `build-core`, `game-files` (with `testkit`, the
synthetic .nup builder) and serde_json.

## Rules it keeps

- Everything it is made of is in this crate, written by hand: no word, address or file of the game.
- The build is `build_driver::build::build`, the one `longshot-builder build` runs, with three test-only seams through
  `Stages` and none in the player's path: the read stage takes the synthetic build id (`read::read_exefs_of`), the
  compile builds the game alone (`Mode::GameOnly`) from a synthetic tree, and the shaders are not prepared.
- The rebuild after one changed instruction must compile that function's unit alone: the test holds the caches'
  keys to it.

## Tests

```
cargo test --release -p synthetic-game -- --ignored
```

The end-to-end test is `#[ignore]`d by default. It needs the pinned clang able to target AArch64, and lld (the
checkout's tools from `mise run tools`; else `LONGSHOT_LLVM`, `EMSDK` or the
PATH). Without them it says so and passes, unless `LONGSHOT_E2E=1` (as CI sets) makes that a failure.
`LONGSHOT_E2E_SCRATCH` names its folder (default: cargo's, on disk beside the target). Linux only. The unit tests
check the NSO conversion and that the one-instruction change changes the result. On a shared PC, run it through the
job queue ([share a PC](../../../docs/dev/developing.md#share-a-pc-the-job-queue)).

See [building and checking the builder](../../../docs/dev/reference/builder.md#building-and-checking-the-builder).
