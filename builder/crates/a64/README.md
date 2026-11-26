# a64

The AArch64 decoder: one 32-bit instruction word to a typed `Insn`, the form the lifter's semantics lower and
discovery walks. It is the first step of the lift: it takes words from the image and gives instructions; it reads no
file and depends on no other crate.

## Main parts

- [`src/lib.rs`](src/lib.rs): `Insn` and its operand types (`Reg`, `Cond`, `Shift`, `Extend`, `Addr` ...); `Insn::flow`
  (a `Flow`: what the instruction does to control flow), `Insn::writes`, `Insn::adr_target`.
- [`src/decode.rs`](src/decode.rs): `decode(word) -> Option<Insn>`, after the Arm ARM's top-level encoding table (C4.1)
  and each group's; [`src/decode/simd.rs`](src/decode/simd.rs) the floating-point and Advanced SIMD groups.
- [`src/simd.rs`](src/simd.rs): the FP and SIMD operand types and operations (`VReg`, `Shape`, `Arr`, `Simd3Op` ...).
- [`src/kind.rs`](src/kind.rs): `Kind`, an instruction as discovery and the sites see it (a branch with its absolute
  target, or a plain operation).
- [`src/bitmask.rs`](src/bitmask.rs): `decode_bit_masks`, the logical immediates (the Arm ARM's DecodeBitMasks).
- [`src/print.rs`](src/print.rs), [`src/print/simd.rs`](src/print/simd.rs): `Display`, the text LLVM's disassembler
  writes with `-M no-aliases`, for the check against llvm-mc and the lifter's messages.

## Used by / uses

Used by `cfg`, `sem`, `sites`, `edits`, `lift` and `longshot-builder` (and `passes`' tests). Uses nothing.

## Rules it keeps

- It decodes what Cortex-A78C runs at EL0: Armv8.2-A with LSE, RCpc, FlagM, PAuth and CRC32, and FP16, RDM, the dot
  product, AES, SHA1, SHA256 and PMULL. A word left unallocated, or one that needs a feature the A78C lacks (MTE, SB,
  RCpc-immo, MOPS, LS64, AltNZCV, SVE, SME), is `None`.
- Fields are named as the Arm ARM names them (`sf`, `rd`, `rn`, `rt2`, `immr` ...). Branch and literal offsets are bytes
  from the instruction; memory offsets are bytes, already scaled.
- Should-be-one and should-be-zero fields decode whatever they hold, as LLVM and the hardware do, except where LLVM
  checks them too (CAS's and CASP's Rt2).
- `Display` stays byte for byte llvm-mc's text: the decoder's check depends on it.

## Tests

```
cargo test -p a64
```

The unit tests cover the bitmask immediates, the kinds and a few decodes. `tests/llvm_mc.rs` checks the decoder
against `llvm-mc --disassemble -mcpu=cortex-a78c -M no-aliases` word for word: random words in every encoding group,
both must agree on whether each is an instruction and on its text. It uses the llvm-mc that `LLVM_MC` names, else the
one on PATH, and passes saying so with neither. `LIFTER_WORDS=<n>` sets the words per group (40000 by default).
`tests/test_lifter.py` runs it with the rest of the workspace.

See [the parts](../../../docs/dev/reference/builder.md#the-parts) and
[what it decodes](../../../docs/dev/reference/builder.md#what-it-decodes).
