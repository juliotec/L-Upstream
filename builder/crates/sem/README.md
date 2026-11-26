# sem

The instructions' semantics: each decoded `a64::Insn` that is not a branch, as mid IR appended to a block. The
function builder (`lift`) calls it for every instruction it lifts; it takes an instruction and its address and gives
`mir` operations, or a `Fault` that says why there are none.

## Main parts

- [`src/lib.rs`](src/lib.rs): `lower(&mut Function, block, pc, &Insn) -> Result<(), Fault>`; `Sem`, the lowering's
  state, whose `cond` the function builder uses for its branches; `Fault` (`Unsupported`: not covered yet; undefined
  here: an unknown system register, an EL1 operation).
- [`src/int.rs`](src/int.rs): data processing, immediate and register.
- [`src/mem.rs`](src/mem.rs): loads, stores, exclusives and atomics; [`src/simdmem.rs`](src/simdmem.rs) LD1 ... LD4,
  ST1 ... ST4.
- [`src/sys.rs`](src/sys.rs): hints, barriers, PSTATE, the system registers and operations EL0 has.
- [`src/fp.rs`](src/fp.rs): floating point as the Arm ARM defines it (FPAdd, FPMulAdd, FPProcessNaNs ...), from IEEE
  arithmetic rounded to nearest; [`src/fpscalar.rs`](src/fpscalar.rs) the scalar FP instructions.
- [`src/fpfast.rs`](src/fpfast.rs): the float operations' fast paths: the host's IEEE operation inline, the exact
  semantics out of line (`Op::Checked`).
- [`src/simd.rs`](src/simd.rs): Advanced SIMD data processing, lane by lane on LLVM vectors;
  [`src/crypto.rs`](src/crypto.rs) AES and SHA; [`src/vreg.rs`](src/vreg.rs) the SIMD and FP registers as operands.

## Used by / uses

Used by `lift` (and `passes`' tests). Uses `a64` and `mir`.

## Rules it keeps

- **The Arm ARM's semantics**, written from the architecture. Where it leaves the choice to the system they follow
  the host's model: pointer authentication is the identity; the counter frequency, cache type and ZVA block are
  constants; a store through SP marks no page; CAS marks its page whether it swapped or not.
- **The float model**: round to nearest (FPCR.RMode not read), FPCR.FZ and FPCR.DN read at each instruction, Arm's
  NaNs made explicitly, FPSR.QC set by the saturating operations, the other cumulative flags not raised. FP16
  arithmetic goes through single precision; its fused forms, estimates and fixed-point conversions are `Unsupported`
  (the guest stops there; the build goes on).
- **A fast path is exact**: the host's IEEE result under its default MXCSR is the architecture's unless the result is
  a NaN or, under FZ, a value is subnormal; the check sends those to the exact helper.
- A `Fault` never fails the build: the function builder ends the block in a trap at that address, so the guest stops
  there if it ever runs it, as at an undefined instruction.
- It is in the lifter's key (`lift/key_crates.rs`): a change here is a new lift.

## Tests

```
cargo test -p sem
```

The unit tests are structural: every word the decoder accepts (and every SIMD word) lowers to IR the verifier
accepts or says why not; a few forms are checked by what they read and write. What the IR computes is checked against
Unicorn by `tests/test_lifter_semantics.py` (game-free: every word generated, each lifted through `longshot-builder
dev words`, compiled and run beside Unicorn; it needs unicorn, cargo and clang++). On a shared PC it goes through the job
queue ([share a PC](../../../docs/dev/developing.md#share-a-pc-the-job-queue)):

```
python -I scripts/dev.py queue run semantics --mode build --desc "lifter semantics" -- \
  python -I tests/test_lifter_semantics.py --cargo cargo --cxx clang++
```

Its docstring lists what it leaves out and why. See [floating point](../../../docs/dev/reference/builder.md#floating-point)
and [building and checking the builder](../../../docs/dev/reference/builder.md#building-and-checking-the-builder).
