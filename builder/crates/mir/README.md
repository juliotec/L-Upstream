# mir

The lifter's mid IR: one guest function as blocks of typed SSA values over the guest state's locations. `sem` and
`lift` build it from the decoded instructions, `edits` and `passes` change it, and `llvm` writes it as LLVM IR.

## Main parts

- [`src/lib.rs`](src/lib.rs): the IR. `Function` (its `entry`, its blocks, `push`, `terminate`, `helpers`, `written`,
  `verify`), `Block`, `Inst`, `Value`, `BlockId`; `Op` (the operations: `Get`/`Set` of a `Loc`, `Load`/`Store`, `Mark`,
  `Checked` ...), `Term` and `Edge` (how a block ends), `Type` and `Elem`, `Loc` and `Flag`, `Helper`, `Global`,
  `Callee`. `Display` is the text dump.
- [`src/verify.rs`](src/verify.rs): `Function::verify` and `verify_pieces`, the IR's rules checked (`VerifyError`).
- [`src/tables.rs`](src/tables.rs): the constants of the reciprocal square root estimate and of AES, worked out from
  their definitions.

## Used by / uses

Used by `sem`, `edits`, `passes`, `llvm`, `lift` and `longshot-builder`. Uses thiserror.

## Rules it keeps

- **No value crosses a block.** Every value is defined once and used only in its block; what flows between blocks
  and between guest instructions is guest state, in `Loc`ations (`Op::Get`, `Op::Set`). So the IR needs no phis, and
  an edit (a hook site, a fast path) or a split cannot break SSA. The verifier checks it.
- Guest memory is one flat window; a store the GPU may read marks its pages with an explicit `Op::Mark` (so a pass can
  merge marks); a store through SP marks none.
- An `Edge` that goes back (to an address at or below the branch's) polls first, so a loop never keeps its core.
- A fast path is an `Op::Checked`: the fast value inline, and a condition under which the value is a `Helper`'s, a
  pure function written once per module.
- The verifier also refuses what LLVM would refuse or make poison of.
- It is in the lifter's key (`lift/key_crates.rs`): a change here is a new lift.

## Tests

```
cargo test -p mir
```

The verifier catches each kind of fault and what LLVM would refuse; pieces name pieces and entries that exist; fast
paths and helpers are checked; a function builds and prints; the tables' constants match their definitions.

See [the parts](../../../docs/dev/reference/builder.md#the-parts) and
[the whole lift](../../../docs/dev/reference/builder.md#the-whole-lift).
