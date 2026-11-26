# passes

Transformations of the mid IR: what the lifter does to a function between the semantics (`sem`, through `lift`) and
the LLVM IR writer (`llvm`). Each pass takes a `mir::Function` and gives it back smaller or cut, with the same
behaviour.

## Main parts

In the order the lifter runs them:

- [`src/marks.rs`](src/marks.rs): `share_marks`: the page marks of stores through one value of a base register, at
  constant offsets in one straight run (no call, SVC, fence or atomic between) and spanning 4 KB or less, become one
  mark over the span (`MarkStats`).
- [`src/simplify.rs`](src/simplify.rs): `simplify`, made of `forward` (a location read after it was written or read
  in the block is that value), `share_pure` (a pure op computed again is the first one's value), `dead_writes` and
  `dead_values` (`SimplifyStats`).
- [`src/split.rs`](src/split.rs): `split`: a function too big for the compiler cut at block boundaries into pieces,
  each a function of its own, an edge between pieces going the way a tail call does (`SplitOptions`, `cost`).

## Used by / uses

Used by `lift` and `longshot-builder`. Uses `mir`; its tests use `a64` and `sem`.

## Rules it keeps

- **A pass changes no behaviour.** Forwarding stops at a call or SVC; a call through the context or an exit to the
  host reads and writes everything; a write read in another block lives.
- A split is exact because a value lives in its own block (the mid IR's rule): only guest state crosses a cut, and a
  cut goes where the fewest edges cross, never through a loop.
- The lifter's IR switches turn passes off for comparison (`longshot-builder dev lift --no-simplify`, `--mark-each`).
- It is in the lifter's key (`lift/key_crates.rs`): a change here is a new lift.

## Tests

```
cargo test -p passes
```

[`src/tests.rs`](src/tests.rs) runs the passes on functions lowered from assembled instructions (none from the game):
which stores share a mark and what ends a run, what forwarding and dead writes keep across blocks and calls, the
float instructions' shared FPCR reads, a small function not split and a big one split into a trampoline and pieces.
`tests/test_lifter_semantics.py` lifts every word through the passes too.

See [the parts](../../../docs/dev/reference/builder.md#the-parts) and
[the whole lift](../../../docs/dev/reference/builder.md#the-whole-lift) (step 3, the units).
