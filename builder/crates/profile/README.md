# profile

The guest profile: how often the game's functions, their blocks and their conditional branches ran in play, keyed by
the guest code itself. The lift reads it (`assets/profiles/guest.profile.xz`) to place and optimise the code: branch
weights and entry counts in the IR, cold functions, hot units at `-O2`. An instrumented lift writes the map its counters
are read back by.

## Main parts

All of it is [`src/lib.rs`](src/lib.rs):

- `Profile`: `load(path)` (plain text or xz), `parse`, `to_text`, `merge` (runs add up), `function(entry, hash)`,
  `never_ran(build, entry, hash)`.
- `Function`: a function's entry count, its blocks' counts and its branches' taken counts (`weight`).
- `code_hash(words)`: a function's key, a hash of its instruction words.
- `Map`, `Counted`, `Counter`: an instrumented build's counters and what each counts (`profile_map.txt`);
  `Map::read_raw` turns a run's raw counts into a `Profile`.
- `MAGIC`, `FORMAT`: the text's first line and version; `Error`.

## Used by / uses

Used by `llvm`, `lift` and `longshot-builder` (`dev profile`). Uses lzma-rs and thiserror.

## Rules it keeps

- **Keyed by the guest code**: a function by its entry and `code_hash` of its words, a block by its first
  instruction's address. A function whose words changed (a game update, an ExeFS patch) no longer matches: its counts
  are left out, never put on other code. On a merge a changed function takes the newer counts alone.
- `never_ran` holds only for the build the profile was recorded on.
- **No game code in it**: the text holds addresses, hashes and counts only, so it can be committed.
- The profile's bytes are in the lift's key, and this crate in the lifter's (`lift/key_crates.rs`).

## Tests

```
cargo test -p profile
```

[`src/tests.rs`](src/tests.rs), on made-up counts: the text reads back as written, counts add up for the same code and
a changed function takes the newer, `never_ran` only on its build, raw counts through a map, the hash follows every
word, the xz file reads.

See [the guest profile](../../../docs/dev/reference/builder.md#the-guest-profile) and how to
[record it](../../../docs/dev/reference/build.md#record-the-guest-profile).
