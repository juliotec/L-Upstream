# llvm

The lifter's LLVM IR writer: mid IR functions as one textual LLVM module (a unit's `.ll` text) that the pinned clang
compiles. It is the lift's last step on a function: `lift` adds each function of a unit, and the compile
(`build-compile`) is given the text, from a file or made on demand.

## Main parts

- [`src/lib.rs`](src/lib.rs): `Module` (`Module::new(Options)`, `add(&Function, Runs)`, `finish() -> String`;
  `instrumented` for a `--profile-generate` unit and its `counted` counters; `set_source_filename`); `Options`
  (polls, the GPU page bitmap, the memory base, callee-saved registers, `count_checks`); `Runs` (a function's code hash, its profile counts, whether it never ran); `ctx`, the byte offsets of
  the context's fields.
- [`src/sync.rs`](src/sync.rs): which locations move between their slots and the context, and where (two dataflow
  passes, so a call does not spill and reload every register).

## Used by / uses

Used by `lift` and `longshot-builder`. Uses `mir` and `profile` (code hashes and counters).

## Rules it keeps

- **The ABI**: every guest function is `void f_<entry>(Ctx *)` over the context of `builder/rt/a64.h`; a direct call is a
  C call, a tail call is `musttail`, an indirect call or jump goes through a two-slot site cache as `A64_ICALL` /
  `A64_IJUMP` do. `ctx` and the runtime's declarations must match `rt/a64.h` (whose `GuestContext` asserts the layout):
  a change on one side is a change on the other.
- The context parameter is `noalias`: no guest memory access can touch it.
- A fast path's helper is written once per module, `internal`, `noinline`, `cold`, reading no memory; the branch to it
  is weighted unlikely.
- The same functions and options give the same text: the compile keys a unit's object by its text's content.
- It is in the lifter's key (`lift/key_crates.rs`): a change here is a new lift.

## Tests

```
cargo test -p llvm
```

[`src/tests.rs`](src/tests.rs) checks the text: the ABI and the sync, vectors, callee-saved registers, a fast path
around its helper, polls on piece edges, GPU-bitmap marks, thread-local globals, NaN constants, an instrumented
module's counters, a profile's branch weights and entry counts. Where clang is found it checks that LLVM accepts the
text; without clang those tests fail, unless `LONGSHOT_NO_CLANG=1` says to pass over them.

See [the parts](../../../docs/dev/reference/builder.md#the-parts), [the gen/ folder](../../../docs/dev/reference/builder.md#the-gen-folder)
and [the runtime](../../../docs/dev/reference/builder.md#the-runtime).
