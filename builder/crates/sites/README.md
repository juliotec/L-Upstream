# sites

The game's [hook sites](../../../docs/dev/reference/hooks.md) as the lifter reads and places them: the tables in
`game/` read and checked, each site found in the game's code exactly once, and placed against the lift's
functions. It takes the tables, the image and the discovery's plan, and gives a `Placed` per site, data that `edits`
turns into mid IR.

## Main parts

- [`src/lib.rs`](src/lib.rs): `Tables` (where the tables are), `load` (the definitions joined with a build's
  locations), `apply` (all of it for an image and a plan, the host's list checked too), `ResolveError`.
- [`src/table.rs`](src/table.rs): the format: `game/sites/sites.toml` (each site's definition) joined with
  `game/sites/<build id>/locations.toml`, every rule checked before anything is placed (`parse_definitions`,
  `Site`, `ident`, `is_name`).
- [`src/signature.rs`](src/signature.rs): `Signature`, `MaskedWord`: a few instruction words with masked nibbles or
  bits.
- [`src/resolve.rs`](src/resolve.rs): `resolve` (a signature's place, from the module's code alone) and `locate` (with
  the lift's functions: another site's function, an anchor, a fingerprint).
- [`src/place.rs`](src/place.rs): `place`: each found site checked against the lift and its form (`Placed`, `Point`,
  `Hook`, `Placement`).
- [`src/host.rs`](src/host.rs): `HostList`, the host's list `game/sites.def` (a flag and a callback slot per site,
  the values the inline rules read).
- [`src/code.rs`](src/code.rs): what the resolver reads of the game: a module's code and the lift's functions.

## Used by / uses

Used by `edits`, `lift`, `build-driver` (its `clashes.rs` refuses the mods' ExeFS patches where they would stop a
site) and `longshot-builder` (`dev sites`). Uses `a64`, `cfg`, `image`, thiserror and toml.

## Rules it keeps

- **A hook is never placed by guess**: a signature, anchor or fingerprint found nowhere or more than once stops the
  lift naming the site (a signature found nowhere names its nearest misses).
- A fault in a table stops with the file, the site and the rule.
- Every site has its line in `game/sites.def`, and every rule's value is there, or the lift stops (it would not
  link).
- Nothing here matches lifted text: it reads instruction words and the plan only.
- The tables hold names, offsets and a few masked words per site, nothing else of the game
  ([what a table may hold](../../../docs/dev/reference/hooks.md#what-a-table-may-hold)).
- Its sources are in `EDITS_SOURCES`, in the whole lift's key, and each function's edits' key holds its sites
  (`lift/key_crates.rs`).

## Tests

```
cargo test -p sites
```

[`src/tests.rs`](src/tests.rs) and [`src/table/tests.rs`](src/table/tests.rs) cover the reader's rules, the resolver, the anchors, the fingerprints and each form's
placement on made-up code. [`tests/repository.rs`](tests/repository.rs) reads the repository's own tables: every
build's locations with its definitions, every site with its host line. On a work folder,
`longshot-builder dev sites --image <dir>/image --tables game` places them without a lift.

See [how a site is placed](../../../docs/dev/reference/hooks.md#how-a-site-is-placed),
[the format](../../../docs/dev/reference/hooks.md#the-format) and [adding a site](../../../docs/dev/reference/hooks.md#adding-a-site).
