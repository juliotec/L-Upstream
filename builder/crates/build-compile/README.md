# build-compile

The builder's compile stage: the lift's gen/ folder and the tree's sources into the game's executable and the
renderer's module (`longshot_vk.so` / `longshot_vk.dll`). It finds the tools, plans every unit with its flags, takes
each object the store already holds by its key, compiles the rest under the memory gate, the longest first, and links.

## Main parts

- [`src/lib.rs`](src/lib.rs): `run(&Options, &Reporter) -> Outcome` (`Built`, or a `Failure` with its `FailedUnit`s
  and what `Stopped` it); `preview` (what a compile would do); `compile_all`.
- [`src/options.rs`](src/options.rs): `Options`, `Mode` (the game and renderer, the game alone, the renderer alone, a dry run, the third-party objects), `Settings` (the switches, read once by
  `Settings::from_env`), `IrText` / `IrSource` (a lifted unit's text made on demand).
- [`src/plan.rs`](src/plan.rs): every unit with its compiler and flags, and the two links.
- [`src/key.rs`](src/key.rs): an object's key; [`src/store.rs`](src/store.rs): the object store, by key, and per base
  key the headers each compile read; [`src/state.rs`](src/state.rs): what the last build learned
  (`compile_state.json`).
- [`src/schedule.rs`](src/schedule.rs): the scheduler under the memory gate; [`src/compile.rs`](src/compile.rs): one
  unit; [`src/link.rs`](src/link.rs): the links, each skipped when nothing it reads changed.
- [`src/toolchain.rs`](src/toolchain.rs): clang, the target, the sysroot, the CPU; [`src/packages.rs`](src/packages.rs):
  the libraries and headers from outside the tree.
- [`src/prebuilt.rs`](src/prebuilt.rs): the release's third-party objects; [`src/vk_profile.rs`](src/vk_profile.rs):
  the renderer's play profile; [`src/generated/`](src/generated/mod.rs): files the build writes itself (the logo,
  the fonts, ReShade's adapted preprocessor, the Windows resources, the version line, the shader stamp), each only when
  its bytes change.

## Used by / uses

Used by `build-driver` (and `synthetic-game`'s test). Uses `build-core`, lzma-rs, miniz_oxide, serde, serde_json and
sha1_smol.

## Rules it keeps

- **An object's key is exactly what changes it**: the key scheme, the compiler's identity (its version and its
  programs' bytes, not its path), the flags (folders that differ between PCs named by what they are), the source's
  content and every header the compile read, by content. A lifted unit's key starts from the content hash `units.txt`
  gives.
- **No build-wide input is in a key**: no path, no builder version, no whole-build option. A change to one function
  compiles its one unit; a new release with the same units and tree compiles only `host/version.c` and
  `gpu/vk/vk_version.cpp`.
- An unchanged rebuild compiles nothing and links nothing.
- Nothing below `options.rs` reads the environment: the switches are read once into `Settings`.
- The shader stamp (`generated/shader_stamp.rs`) hashes the shader translator's sources, C and C++ by their tokens (a
  comment changes nothing). A new stamp drops every player's shader caches.

## Tests

```
cargo test -p build-compile
```

The unit tests cover keys, the store, the state, the scheduler, the plan, the links' records, the toolchain and the
renderer's profile. [`tests/compile_link.rs`](tests/compile_link.rs) compiles and links two C files and a header with the
PC's clang (and passes without one): an unchanged rebuild compiles nothing, a changed source or header what it
touches, another work folder with the same store nothing; a lifted unit's text made on demand compiles to its file's
object. `cargo run -p build-compile --example plan -- --work <dir>` prints every unit it would compile and its flags.

See [the compile](../../../docs/dev/reference/builder.md#the-compile), [build switches](../../../docs/dev/reference/build.md#build-switches),
[the compiler memory gate](../../../docs/dev/reference/build.md#the-compiler-memory-gate) and
[what builds again](../../../docs/dev/reference/build.md#what-builds-again).
