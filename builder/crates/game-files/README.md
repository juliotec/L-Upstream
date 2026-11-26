# game-files

The player's game files, as the builder's read stage reads them: the .nup (a decrypted Switch 2 dump) listed,
verified, its exefs unpacked and its romfs indexed for the game to read in place; the code modules loaded; the dump
checked; the mods' ExeFS patches; the game's title and icon; the disk. It takes a .nup or a dump folder and gives the
exefs, `longshot_romfs.idx` and the checks' answers to `build-driver` and `image`.

## Main parts

- [`src/nup.rs`](src/nup.rs): the .nup package: its TOC, titles, exefs and romfs trees, verified and read.
- [`src/romfs.rs`](src/romfs.rs): the romfs index the game folder gets (its format is documented here), `Index` for the
  builder's later stages, and `RECIPE`, the rules an index was made by.
- [`src/nso.rs`](src/nso.rs): the code modules (NSO): loaded for the image (LZ4 and ZBIC segments, RELR relocations)
  or unpacked in place.
- [`src/zstd.rs`](src/zstd.rs): zstd and ZBIC frames decoded whole (ZBIC: the modules' compression from firmware
  22.0.0 on, written from its description; the .nup's own streamed frames go through ruzstd).
- [`src/checks.rs`](src/checks.rs): what a dump must be before a build starts, and `check`'s answer.
- [`src/patches.rs`](src/patches.rs): the manifest mods' IPS, IPS32 and IPSwitch patches for main: collected from the
  mods the Mod API's layer gives, in their order, and applied (one that clashes with a hook site is refused by
  `build-driver`'s `clashes.rs`, which reads the sites' tables).
- [`src/mod_layout.rs`](src/mod_layout.rs): the mods in a profile left in the emulators' layout (bare `romfs`, `exefs`,
  `cheats`, no `mod.json`), which `Longshot.exe mod import` turns into manifest mods: the `mod_needs_import` warning.
- [`src/title.rs`](src/title.rs): the game's name and icon from its .nup; [`src/disk.rs`](src/disk.rs): free space,
  the file system, whether a folder is on a hard drive.
- [`src/le.rs`](src/le.rs): little-endian fields read without trusting their place.
- [`src/testkit.rs`](src/testkit.rs) (feature `testkit`): a synthetic .nup built in memory, for this crate's tests and
  `synthetic-game`'s.

## Used by / uses

Used by `image`, `build-assets`, `build-driver` and `synthetic-game` (with `testkit`). Uses `build-core`, lz4_flex,
ruzstd, serde, serde_json, sha2; libc on Unix, windows-sys on Windows.

## Rules it keeps

- **Every input is untrusted.** A field past the bytes is `None`; a module's sizes are bounded (`nso::MAX_MODULE`)
  before anything is allocated; a zstd block decodes to no more than its maximum nor past its limit; a damaged or
  hostile patch or .nup is said as an error, never a panic or a huge allocation.
- Every failure a player can act on is a `build_core::Error::Player` with a stable code.
- `ROMFS_SOURCES` ([`build.rs`](build.rs), the files [`recipe_hash.rs`](recipe_hash.rs) names) is a hash of the sources
  that decide what an index holds, in `romfs::RECIPE`: an index made by other code is made again. A source that changes
  what an index holds belongs in that list.
- `unsafe` only in `disk.rs`'s system calls (`statvfs`, the Windows volume queries), allowed there alone.
- Nothing of the game is in the crate: the tests' files are synthetic (`testkit`, `testdata/`).

## Tests

```
cargo test -p game-files
```

The .nup listed and verified, the romfs index written and read, NSO loading and its bounds, zstd and ZBIC frames
(against frames the zstd command made, `testdata/`), the patch formats, the checks, the title, the
disk. The examples: `nup`, a .nup listed, verified, extracted or indexed by hand; `mods`, for `tests/test_mods_list.py`.

See [the romfs, read in place](../../../docs/dev/reference/builder.md#the-romfs-read-in-place),
[a build](../../../docs/dev/reference/builder.md#a-build) (step 2, read), [ExeFS patch mods](../../../docs/dev/reference/hooks.md#exefs-patch-mods)
and [the events' codes](../../../docs/dev/reference/build-events.md#codes).
