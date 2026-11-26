# cfg

Discovery, the lift's first step: the game's functions and their control flow, found from the image alone. It takes
an `image::Image` and gives a `cfg::plan::Plan`, the lift plan: every function (an entry and every instruction reachable from it without
crossing another entry), each BR's jump table, the PLT stubs seen through, and the setjmp owners, whose calls' return
addresses become entries of their own.

## Main parts

- [`src/lib.rs`](src/lib.rs): `discover(&image, inputs) -> Plan`.
- [`src/plan.rs`](src/plan.rs): the plan, plain data that the function builder (`lift`), the hook sites (`sites`) and
  the edits build from. `Plan`: the image it was found in, the code ranges, the entries, the host's hooks, the
  `functions` by entry, the `jump_tables` by BR address, the `plt` stubs, the `setjmp_owners` and the devirtualised
  calls (`devirt`); `Plan::callee` follows PLT stubs to the function they jump to, `holders` and `function_at` say
  which functions hold an address. `Function`: an entry's instructions as runs of consecutive addresses (`runs`) and
  its labels, the branch targets inside it (`from_pcs` makes the runs; `pcs`, `contains`, `len` read them). `Plt`: a
  stub's target and the GOT slot it reads.
- [`src/entries.rs`](src/entries.rs): where discovery starts. `inputs(&image)` gathers the entry candidates of every
  module (code start, init and fini, unwind starts, exports, relocated code addresses, BL targets, the init array),
  the code addresses taken with ADR or ADRP+ADD, the host's hooks and the symbol names.
- [`src/module.rs`](src/module.rs): what each module says of itself: its MOD0 header and ELF dynamic section, and
  through them its symbols, relocations, init array and `.eh_frame_hdr` table (`read`, `fde_spans`).
- [`src/walk.rs`](src/walk.rs): `Discovery`, `Inputs`, `Stats`: the walks, the jump tables tried again once every
  function's are known, the PLT stubs, the setjmp owners.
- [`src/table.rs`](src/table.rs): jump tables, found by running the straight-line code before a BR symbolically
  (LLVM's AArch64 form: `base + (entry << k)` from a constant table).
- [`src/classify.rs`](src/classify.rs): `Kind`, what the walk needs of an instruction, from `a64`.

## Used by / uses

Used by `lift` (`lift::whole::discover`) and `longshot-builder` (`dev discover`, `dev sites`); its plan by `sites`,
`edits` and `lift` too. Uses `a64`, `image` and thiserror.

## Rules it keeps

- A function is an entry and every instruction reachable from it without crossing another entry (a64-kit's model).
  Code two functions share is in both.
- **The order is fixed**: entries in address order, each function's instructions in its walk's order, then the
  tables missed tried again. A table found by the predecessors' search sees the tables found so far in its function's
  unwind range, so the result depends on that order; a change to it can change the plan.
- Nothing is read relocated from the module tables (they hold module offsets); a relocated value is read from the
  image at the relocation's place.
- It is in the lifter's key (`lift/key_crates.rs`): a change here is a new lift.

## Tests

```
cargo test -p cfg
```

`tests/discovery.rs` runs discovery on a made-up image (instructions assembled by hand, nothing from the game): a
bounded jump table, a call to `setjmp`, a PLT stub through a GOT slot; an entry inside a function cuts its walk; an
unbounded table reads until a target leaves the function; an index masked to two bits bounds its table at four.
`src/plan.rs`'s own tests: `from_pcs` makes the runs from scattered addresses; an address belongs to the function that
holds it. On a
work folder, `longshot-builder dev discover --image <dir>/image` gives its counts and time.

See [the whole lift](../../../docs/dev/reference/builder.md#the-whole-lift) (step 1, discovery).
