# rt

The lifter's runtime, in C: the guest model the lifted code and the host share, and what the lifted IR calls. The
lift (`lift::whole`) embeds these files and writes them into gen/rt/ with `a64_config.h`, the lift's own choices, so
the host and the lifted code are always compiled against the same model. The compile builds `a64_rt.c` and `a64_fp.c`
into the game (at `-O2` by default), and the host's C (`host/`, `gpu/`) includes `a64.h` with `-I gen/rt`.

## Main parts

- [`a64.h`](a64.h): the guest context (`GuestContext`: X0-X30 and SP, NZCV, the V registers, TPIDR_EL0, FPCR, FPSR,
  the exclusive monitor), guest memory and its 64-bit window, the page marks (`A64_MARK`), polls (`A64_POLL`),
  `recomp_load*` / `recomp_store*`, `A64_ICALL` / `A64_IJUMP`, and the runtime's entry points.
- [`a64_rt.c`](a64_rt.c): the function lookup (`a64_lookup`, over gen/table.c), the indirect calls and jumps with their
  per-site caches (`a64_icall_site`, `a64_ijump_site`), traps, the exclusive monitor (`recomp_ldxr` ...
  `recomp_clrex`), the counter (`recomp_cntpct`), the setjmp calls, `a64_dirty`. The OS layer overrides its weak
  hooks: `a64_svc`, `a64_on_trap`, `a64_call_miss`, `a64_lookup_host`, `a64_ijump_hook`, `a64_cntpct_host`,
  `a64_unimplemented`.
- [`a64_fp.c`](a64_fp.c): the exact single-precision helpers host code calls in place of guest instructions
  (`recomp_fpe_fma32`, `recomp_fpe_div32`), under the lifter's float model.
- [`PROVENANCE.md`](PROVENANCE.md): where it comes from (a64-kit's `rt/`, with Hookshot's and Longshot's changes).

## Used by / uses

Embedded by `lift` (`src/whole/files.rs`) and compiled by `build-compile` (`plan.rs`, the lift's C). The IR that
`llvm` writes does not include `a64.h` but has its layout and calls its symbols (`llvm/src/lib.rs`: `ctx` and
`RUNTIME`). `synthetic-game`'s test host links it too.

## Rules it keeps

- **The context's layout is shared with the IR writer**: `a64.h` asserts the offsets (`_Static_assert`) that `llvm`'s
  `ctx` gives; a change to one is a change to the other.
- None of the lift's choices (the window's base, the marks' table, the GPU page bitmap, polls) is defined on the
  command line: they come from `a64_config.h`.
- `a64_fp.c` follows `sem/src/fp.rs`: round to nearest, FPCR.FZ and FPCR.DN read from the context, Arm's NaNs, no
  cumulative FPSR flags.
- It is in the lifter's key (`lift/build.rs`): a change here is a new lift.
- It is compiled into the game, which is GPL; TOTKPort's notice for a64-kit is `host/NOTICE.TOTKPort.md`.

## Tests

No tests of its own. `lift`'s whole-lift tests compile it with the lifted units where clang is found
(`LONGSHOT_NO_CLANG=1` passes over them), `tests/test_lifter_semantics.py` runs every covered instruction against it
(its page marks, `a64_dirty`), and `synthetic-game`'s end-to-end test runs a program on it
(`cargo test --release -p synthetic-game -- --ignored`).

See [the runtime](../../docs/dev/reference/builder.md#the-runtime), [the gen/ folder](../../docs/dev/reference/builder.md#the-gen-folder)
and [floating point](../../docs/dev/reference/builder.md#floating-point).
