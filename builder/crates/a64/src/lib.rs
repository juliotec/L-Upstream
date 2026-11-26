//! The AArch64 decoder: one 32-bit instruction word to an [`Insn`], the typed form the lifter's semantics lower.
//!
//! It decodes what the Switch 2's CPU (Cortex-A78C) runs at EL0: Armv8.2-A with LSE, RCpc, FlagM, PAuth and CRC32. A word
//! that architecture leaves unallocated, or that needs a feature the A78C lacks (MTE, SB, RCpc-immo, MOPS, LS64,
//! AltNZCV, SVE, SME), decodes to `None`. Of the floating-point and Advanced SIMD extensions it has FP16, RDM, the dot
//! product, AES, SHA1, SHA256 and PMULL; not FHM, FCMA, JSCVT, I8MM, BF16, SHA3, SHA512, SM3, SM4 or FRINT32/64. Their
//! operand types and operations are the [`VReg`], [`Shape`], [`Simd3Op`] ... below.
//!
//! **Fields are named as the Arm ARM names them**, so an encoding diagram reads straight into a variant: `sf` is the
//! 64-bit (X register) form; `rd`, `rn`, `rm`, `ra` are the destination and the sources; `rt`, `rt2` are the registers a
//! load or store moves and `rs` a status or compare register; `imm*`, `immr`, `imms`, `hw`, `nzcv` and `cond` are the
//! fields of those names. Branch and literal offsets are in bytes from the instruction's own address; memory offsets
//! are in bytes, already scaled.
//!
//! `Display` writes an instruction the way LLVM's disassembler does with `-M no-aliases` (the [`print`] module), so
//! the decoder is checked against llvm-mc word for word (`tests/llvm_mc.rs`).

mod bitmask;
mod decode;
mod kind;
pub mod print;
mod simd;

pub use bitmask::decode_bit_masks;
pub use decode::decode;
pub use kind::Kind;
pub use simd::*;

/// A general register number, 0 to 31. What 31 means (SP or the zero register) is the instruction's: each field says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Reg(pub u8);

impl Reg {
    /// Register 31: SP or the zero register.
    pub const SP_OR_ZR: Reg = Reg(31);

    /// Whether this is register 31.
    pub fn is_31(self) -> bool {
        self.0 == 31
    }
}

/// A condition: the 4-bit field of B.cond, CSEL, CCMP (0 EQ ... 14 AL, 15 NV).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cond(pub u8);

/// A shifted register's shift, as the architecture names them.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shift {
    Lsl,
    Lsr,
    Asr,
    Ror,
}

/// An extended register's extension, as the architecture names them (UXTX is the identity: written LSL).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Extend {
    Uxtb,
    Uxth,
    Uxtw,
    Uxtx,
    Sxtb,
    Sxth,
    Sxtw,
    Sxtx,
}

impl Extend {
    /// The extension an `option` field (3 bits) names.
    pub fn from_option(option: u32) -> Extend {
        [Extend::Uxtb, Extend::Uxth, Extend::Uxtw, Extend::Uxtx, Extend::Sxtb, Extend::Sxth, Extend::Sxtw, Extend::Sxtx]
            [option as usize & 7]
    }

    /// Whether the register it extends is an X register (UXTX, SXTX) rather than a W one.
    pub fn is_64(self) -> bool {
        matches!(self, Extend::Uxtx | Extend::Sxtx)
    }
}

/// The logical operations, by mnemonic (the immediate forms have only AND, ORR, EOR, ANDS).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogicOp {
    And,
    Bic,
    Orr,
    Orn,
    Eor,
    Eon,
    Ands,
    Bics,
}

/// MOVN, MOVZ, MOVK.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveOp {
    Movn,
    Movz,
    Movk,
}

/// SBFM, BFM, UBFM (every bitfield move and extension is one of these).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitfieldOp {
    Sbfm,
    Bfm,
    Ubfm,
}

/// CSEL, CSINC, CSINV, CSNEG.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CselOp {
    Csel,
    Csinc,
    Csinv,
    Csneg,
}

/// The one-source data-processing operations, by mnemonic.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DataProc1Op {
    Rbit,
    Rev16,
    /// REV32, of an X register only: on a W register that encoding is [`DataProc1Op::Rev`].
    Rev32,
    Rev,
    Clz,
    Cls,
}

/// The two-source data-processing operations (the shifts are the register-amount ones: LSLV ...).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DataProc2Op {
    Udiv,
    Sdiv,
    Lslv,
    Lsrv,
    Asrv,
    Rorv,
    Pacga,
}

/// The three-source data-processing operations (multiplies), by mnemonic.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DataProc3Op {
    Madd,
    Msub,
    Smaddl,
    Smsubl,
    Smulh,
    Umaddl,
    Umsubl,
    Umulh,
}

/// A PAuth key: A or B, for instructions (I) or data (D).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PacKey {
    Ia,
    Ib,
    Da,
    Db,
}

/// PACIA and the rest (sign, `auth` false) or AUTIA and the rest (authenticate), with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PacOp {
    /// Authenticate (AUT*) rather than sign (PAC*).
    pub auth: bool,
    /// The key.
    pub key: PacKey,
}

/// What a branch to a register authenticates first (PAuth's BRAA, BLRAAZ, RETAA ...). `b`: the B key, else A.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BranchAuth {
    /// Nothing: a plain BR, BLR, RET.
    None,
    /// The modifier is zero (BRAAZ, BLRAAZ).
    Zero { b: bool },
    /// The modifier is a register (BRAA, BLRAA).
    Reg { b: bool, rm: Reg },
    /// The modifier is SP (RETAA, ERETAA).
    Sp { b: bool },
}

/// The branches to a register, by mnemonic.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BranchRegOp {
    Br,
    Blr,
    Ret,
    Eret,
    Drps,
}

/// The size of a memory access: `1 << log2` bytes (0: a byte .. 4: a Q register's 16 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Size(pub u8);

impl Size {
    /// Its bytes.
    pub fn bytes(self) -> u32 {
        1 << self.0
    }
}

/// What a single-register load or store does with its register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LsKind {
    /// A store of a general register.
    Store,
    /// A zero-extending load into a general register (W for sizes below 8 bytes).
    Load,
    /// A sign-extending load into a W register (`x64` false) or an X one.
    LoadSigned {
        /// Into an X register.
        x64: bool,
    },
    /// A store of a SIMD and FP register.
    StoreFp,
    /// A load into a SIMD and FP register.
    LoadFp,
    /// PRFM: the 5-bit prefetch operation is in the Rt field.
    Prefetch,
}

/// Which of a load's or store's mnemonic families it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LsFlavour {
    /// LDR, STR and their forms.
    Normal,
    /// LDUR, STUR: an unscaled 9-bit offset.
    Unscaled,
    /// LDTR, STTR: an unprivileged access (at EL0, a plain one).
    Unprivileged,
}

/// A load's or store's address. `rn` is the base register (31 is SP); offsets are bytes.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Addr {
    /// `[Rn, #imm]`.
    Offset { rn: Reg, imm: i64 },
    /// `[Rn, #imm]!`: the base is written back first.
    PreIndex { rn: Reg, imm: i64 },
    /// `[Rn], #imm`: the base is written back after.
    PostIndex { rn: Reg, imm: i64 },
    /// `[Rn, Rm{, extend {#amount}}]`: `shifted` is the S bit (the index is shifted by the access size's log2 when set).
    RegOffset { rn: Reg, rm: Reg, extend: Extend, shifted: bool },
    /// PC + `imm`: a literal load.
    Literal { imm: i64 },
}

/// What a load or store pair does with its registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PairKind {
    /// STP, STNP of general registers.
    Store,
    /// LDP, LDNP into general registers.
    Load,
    /// LDPSW: two sign-extended words into X registers.
    LoadSignedWord,
    /// STP, STNP of SIMD and FP registers.
    StoreFp,
    /// LDP, LDNP into SIMD and FP registers.
    LoadFp,
}

/// LSE's atomic memory operations (LDADD and the rest), and SWP.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AtomicOp {
    Add,
    Clr,
    Eor,
    Set,
    Smax,
    Smin,
    Umax,
    Umin,
    Swp,
}

/// The barriers (CRm is their option: SY, ISH ...).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Barrier {
    Dsb,
    Dmb,
    Isb,
}

/// A system register, as its op0:op1:CRn:CRm:op2 fields (the 16 bits MRS and MSR hold).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SysReg(pub u16);

impl SysReg {
    /// The condition flags.
    pub const NZCV: SysReg = SysReg::new(3, 3, 4, 2, 0);
    /// The floating-point control register.
    pub const FPCR: SysReg = SysReg::new(3, 3, 4, 4, 0);
    /// The floating-point status register.
    pub const FPSR: SysReg = SysReg::new(3, 3, 4, 4, 1);
    /// The thread pointer (C++ thread-local storage).
    pub const TPIDR_EL0: SysReg = SysReg::new(3, 3, 13, 0, 2);
    /// The OS's read-only thread pointer (Horizon's thread-local region).
    pub const TPIDRRO_EL0: SysReg = SysReg::new(3, 3, 13, 0, 3);
    /// The virtual counter.
    pub const CNTVCT_EL0: SysReg = SysReg::new(3, 3, 14, 0, 2);
    /// The counter's frequency.
    pub const CNTFRQ_EL0: SysReg = SysReg::new(3, 3, 14, 0, 0);

    /// The register of these fields.
    pub const fn new(op0: u16, op1: u16, crn: u16, crm: u16, op2: u16) -> SysReg {
        SysReg((op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2)
    }

    /// Its fields: (op0, op1, CRn, CRm, op2).
    pub fn fields(self) -> (u16, u16, u16, u16, u16) {
        let v = self.0;
        (v >> 14, (v >> 11) & 7, (v >> 7) & 15, (v >> 3) & 15, v & 7)
    }
}

/// A conditional compare's second operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmpRhs {
    /// A register.
    Reg(Reg),
    /// A 5-bit immediate.
    Imm(u8),
}

/// One decoded instruction (field names: see the crate's docs).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Insn {
    // ---- data processing, immediate
    /// ADR, or ADRP (`page`: the offset is from the instruction's 4 KB page, a multiple of 4096).
    Adr { page: bool, rd: Reg, imm: i64 },
    /// ADD, ADDS, SUB, SUBS with a 12-bit immediate, shifted left 12 when `shift12`. Rd is SP unless `setflags`; Rn is SP.
    AddSubImm { sf: bool, sub: bool, setflags: bool, rd: Reg, rn: Reg, imm12: u16, shift12: bool },
    /// AND, ORR, EOR, ANDS with a bitmask immediate, decoded. Rd is SP unless ANDS.
    LogicalImm { sf: bool, op: LogicOp, rd: Reg, rn: Reg, imm: u64 },
    /// MOVN, MOVZ, MOVK of `imm16` at bit `hw * 16`.
    MoveWide { sf: bool, op: MoveOp, rd: Reg, imm16: u16, hw: u8 },
    /// SBFM, BFM, UBFM.
    Bitfield { sf: bool, op: BitfieldOp, rd: Reg, rn: Reg, immr: u8, imms: u8 },
    /// EXTR: Rn:Rm shifted right by `lsb`.
    Extr { sf: bool, rd: Reg, rn: Reg, rm: Reg, lsb: u8 },

    // ---- branches, exceptions, system
    /// B, or BL (`link`).
    B { link: bool, offset: i64 },
    /// B.cond.
    BCond { cond: Cond, offset: i64 },
    /// CBZ, or CBNZ (`nonzero`).
    Cbz { sf: bool, nonzero: bool, rt: Reg, offset: i64 },
    /// TBZ, or TBNZ (`nonzero`), of bit `bit` of Rt.
    Tbz { nonzero: bool, rt: Reg, bit: u8, offset: i64 },
    /// BR, BLR, RET, ERET, DRPS, and their PAuth forms.
    BranchReg { op: BranchRegOp, rn: Reg, auth: BranchAuth },
    /// SVC: a supervisor call.
    Svc { imm: u16 },
    /// HVC (traps at EL0).
    Hvc { imm: u16 },
    /// SMC (traps at EL0).
    Smc { imm: u16 },
    /// BRK: a breakpoint.
    Brk { imm: u16 },
    /// HLT: a halt (traps at EL0).
    Hlt { imm: u16 },
    /// DCPS1 to DCPS3 (trap at EL0).
    Dcps { level: u8, imm: u16 },
    /// UDF: permanently undefined.
    Udf { imm: u16 },
    /// The hint space (NOP, YIELD, the PAuth hints, BTI ...): CRm:op2.
    Hint { imm: u8 },
    /// CLREX: clear the exclusive monitor.
    Clrex { crm: u8 },
    /// DSB, DMB, ISB.
    Barrier { op: Barrier, crm: u8 },
    /// MSR (immediate): the PSTATE field (op1, op2) set from CRm. CFINV is op1 0, op2 0.
    Pstate { op1: u8, op2: u8, crm: u8 },
    /// SYS, or SYSL (`l`): DC, IC, AT and TLBI are SYS.
    Sys { l: bool, op1: u8, crn: u8, crm: u8, op2: u8, rt: Reg },
    /// MRS: Rt from a system register.
    Mrs { reg: SysReg, rt: Reg },
    /// MSR (register): a system register from Rt.
    Msr { reg: SysReg, rt: Reg },

    // ---- loads and stores
    /// A single-register load, store or prefetch.
    LoadStore { kind: LsKind, flavour: LsFlavour, size: Size, rt: Reg, addr: Addr },
    /// LDP, STP, LDNP, STNP, LDPSW.
    Pair { kind: PairKind, nontemporal: bool, size: Size, rt: Reg, rt2: Reg, addr: Addr },
    /// LDXR, LDAXR, STXR, STLXR and their pair forms (`rt2`): `ordered` is the acquire or release; Rs is a store's
    /// status register.
    Exclusive { load: bool, ordered: bool, size: Size, rs: Reg, rt: Reg, rt2: Option<Reg>, rn: Reg },
    /// LDAR, STLR, or (`limited`) the LORegions forms LDLAR, STLLR.
    Ordered { load: bool, limited: bool, size: Size, rt: Reg, rn: Reg },
    /// LDAPR (RCpc).
    Ldapr { size: Size, rt: Reg, rn: Reg },
    /// CAS (Rs the compare value, Rt the new one), or CASP (`pair`: Rs:Rs+1 and Rt:Rt+1).
    Cas { pair: bool, size: Size, acquire: bool, release: bool, rs: Reg, rt: Reg, rn: Reg },
    /// LDADD and the rest, and SWP: Rs the operand, Rt the old value.
    Atomic { op: AtomicOp, size: Size, acquire: bool, release: bool, rs: Reg, rt: Reg, rn: Reg },
    /// LDRAA, or LDRAB (`b`): Rn authenticated with a D key, then the offset; written back when `writeback`.
    LdrPac { b: bool, writeback: bool, rt: Reg, rn: Reg, imm: i64 },

    // ---- data processing, register
    /// The logical operations with a shifted register.
    LogicalReg { sf: bool, op: LogicOp, rd: Reg, rn: Reg, rm: Reg, shift: Shift, amount: u8 },
    /// ADD, ADDS, SUB, SUBS with a shifted register (never ROR).
    AddSubReg { sf: bool, sub: bool, setflags: bool, rd: Reg, rn: Reg, rm: Reg, shift: Shift, amount: u8 },
    /// ADD, ADDS, SUB, SUBS with an extended register. Rd is SP unless `setflags`; Rn is SP.
    AddSubExt { sf: bool, sub: bool, setflags: bool, rd: Reg, rn: Reg, rm: Reg, extend: Extend, amount: u8 },
    /// ADC, ADCS, SBC, SBCS.
    AddSubCarry { sf: bool, sub: bool, setflags: bool, rd: Reg, rn: Reg, rm: Reg },
    /// RMIF (FlagM): NZCV from Rn rotated right by `shift`, where `mask` says.
    Rmif { rn: Reg, shift: u8, mask: u8 },
    /// SETF8, or SETF16 (`sixteen`) (FlagM).
    Setf { sixteen: bool, rn: Reg },
    /// CCMP, or CCMN (`negate`).
    CondCompare { sf: bool, negate: bool, rn: Reg, rhs: CmpRhs, nzcv: u8, cond: Cond },
    /// CSEL, CSINC, CSINV, CSNEG.
    CondSelect { sf: bool, op: CselOp, rd: Reg, rn: Reg, rm: Reg, cond: Cond },
    /// RBIT, REV16, REV32, REV, CLZ, CLS.
    DataProc1 { sf: bool, op: DataProc1Op, rd: Reg, rn: Reg },
    /// UDIV, SDIV, the register shifts, PACGA.
    DataProc2 { sf: bool, op: DataProc2Op, rd: Reg, rn: Reg, rm: Reg },
    /// CRC32B ... CRC32X, or (`castagnoli`) CRC32CB ... CRC32CX: `size` is the data's (Rm's).
    Crc32 { castagnoli: bool, size: Size, rd: Reg, rn: Reg, rm: Reg },
    /// PACIA ... AUTDB on Rd, with the modifier Rn (`None`: the Z forms, a zero modifier).
    Pac { op: PacOp, rd: Reg, rn: Option<Reg> },
    /// XPACI, or XPACD (`data`).
    Xpac { data: bool, rd: Reg },
    /// The multiplies: MADD ... UMULH.
    DataProc3 { sf: bool, op: DataProc3Op, rd: Reg, rn: Reg, rm: Reg, ra: Reg },

    // ---- scalar floating point (`ty` the operands' format)
    /// FMOV (register), FABS, FNEG, FSQRT, FRINT*.
    Fp1 { op: Fp1Op, ty: FpType, rd: VReg, rn: VReg },
    /// FCVT between formats: Rn of `from` to Rd of `to`.
    FpCvt { from: FpType, to: FpType, rd: VReg, rn: VReg },
    /// FMUL, FDIV, FADD, FSUB, FMAX, FMIN, FMAXNM, FMINNM, FNMUL.
    Fp2 { op: Fp2Op, ty: FpType, rd: VReg, rn: VReg, rm: VReg },
    /// FMADD, FMSUB, FNMADD, FNMSUB.
    Fp3 { op: Fp3Op, ty: FpType, rd: VReg, rn: VReg, rm: VReg, ra: VReg },
    /// FCMP, or FCMPE (`signaling`: a quiet NaN raises Invalid too), of Rn with Rm or (`None`) with +0.0.
    FpCmp { ty: FpType, signaling: bool, rn: VReg, rm: Option<VReg> },
    /// FCCMP, or FCCMPE (`signaling`): the flags of Rn compared with Rm if `cond` holds, else `nzcv`.
    FpCondCmp { ty: FpType, signaling: bool, rn: VReg, rm: VReg, nzcv: u8, cond: Cond },
    /// FCSEL: Rn if `cond` holds, else Rm.
    FpCsel { ty: FpType, rd: VReg, rn: VReg, rm: VReg, cond: Cond },
    /// FMOV (scalar, immediate): `imm8` an FP8 immediate ([`fp_expand_imm`]).
    FpImm { ty: FpType, rd: VReg, imm8: u8 },
    /// FCVT{N,P,M,Z,A}{S,U} to a general register (W, or X when `sf`), or with `fbits` (1 to 64, `round` Zero) FCVTZS
    /// and FCVTZU to fixed point; `fbits` 0 is the integer form.
    FpToInt { ty: FpType, sf: bool, round: Round, unsigned: bool, fbits: u8, rd: Reg, rn: VReg },
    /// SCVTF, UCVTF (`unsigned`) of a general register, from fixed point with `fbits` (0: the integer form).
    IntToFp { ty: FpType, sf: bool, unsigned: bool, fbits: u8, rd: VReg, rn: Reg },
    /// FMOV (general): the bits of `part` from the SIMD and FP register to the general one (`to_general`) or back.
    FmovGen { part: FmovPart, to_general: bool, gpr: Reg, fpr: VReg },

    // ---- Advanced SIMD data processing
    /// The three-same groups (integer, FP, FP16, the RDM and dot product extras).
    Simd3 { op: Simd3Op, shape: Shape, rd: VReg, rn: VReg, rm: VReg },
    /// The three-different group: `shape` the narrow operands' ([`DiffOp`]).
    SimdDiff { op: DiffOp, shape: Shape, rd: VReg, rn: VReg, rm: VReg },
    /// The two-register miscellaneous groups ([`Simd2Op`] says what `shape` is).
    Simd2 { op: Simd2Op, shape: Shape, rd: VReg, rn: VReg },
    /// An operation across the lanes of Rn, to the scalar Rd.
    SimdAcross { op: AcrossOp, arr: Arr, rd: VReg, rn: VReg },
    /// A scalar pairwise operation of the two elements of Rn (of `size`).
    SimdPairwise { op: PairwiseOp, size: Size, rd: VReg, rn: VReg },
    /// A shift by an immediate ([`ShiftOp`]: `shape` the source's).
    SimdShift { op: ShiftOp, shape: Shape, rd: VReg, rn: VReg, shift: u8 },
    /// An operation with element `index` of Rm ([`ElemOp`]: `shape` the first source's).
    SimdElem { op: ElemOp, shape: Shape, rd: VReg, rn: VReg, rm: VReg, index: u8 },
    /// UZP1, UZP2, TRN1, TRN2, ZIP1, ZIP2 (`second`: the …2 form).
    SimdPermute { op: PermuteOp, second: bool, arr: Arr, rd: VReg, rn: VReg, rm: VReg },
    /// EXT: bytes `index` onwards of Rm:Rn (of 16 bytes, or 8 without `q`).
    SimdExt { q: bool, rd: VReg, rn: VReg, rm: VReg, index: u8 },
    /// TBL, or TBX (`tbx`: an index out of range keeps Rd's byte): bytes of the `regs` registers from Rn.
    SimdTable { q: bool, tbx: bool, regs: u8, rd: VReg, rn: VReg, rm: VReg },
    /// DUP (element) to a vector, or (the scalar shape) to a scalar (written MOV): element `index` of Rn.
    SimdDupElem { shape: Shape, rd: VReg, rn: VReg, index: u8 },
    /// DUP (general): the low bits of a general register to every element.
    SimdDupGen { arr: Arr, rd: VReg, rn: Reg },
    /// INS (element): element `src` of Rn to element `dst` of Rd, both of `size`.
    SimdInsElem { size: Size, rd: VReg, dst: u8, rn: VReg, src: u8 },
    /// INS (general): the low bits of a general register to element `index` of Rd.
    SimdInsGen { size: Size, rd: VReg, index: u8, rn: Reg },
    /// UMOV, or SMOV (`signed`): element `index` of Rn to a W register, or an X one (`x64`).
    SimdMovToGen { signed: bool, x64: bool, size: Size, rd: Reg, rn: VReg, index: u8 },
    /// MOVI, MVNI, ORR, BIC, FMOV (vector, immediate).
    SimdModImm { imm: ModImm, rd: VReg },
    /// AESE, AESD, AESMC, AESIMC.
    Aes { op: AesOp, rd: VReg, rn: VReg },
    /// SHA1C ... SHA256SU1.
    Sha3 { op: Sha3Op, rd: VReg, rn: VReg, rm: VReg },
    /// SHA1H, SHA1SU1, SHA256SU0.
    Sha2 { op: Sha2Op, rd: VReg, rn: VReg },

    // ---- Advanced SIMD loads and stores
    /// LD1 ... LD4, LD1R ... LD4R, ST1 ... ST4, from Rt (and the registers after it, modulo 32) at Rn (SP for 31),
    /// written back by `post` when it has one.
    SimdLdSt { load: bool, what: Structure, rt: VReg, rn: Reg, post: Option<PostIndex> },
}

/// What an instruction does to control flow: what discovery walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flow {
    /// Goes on to the next instruction.
    Next,
    /// B: always to `offset`.
    Jump {
        /// Bytes from the instruction.
        offset: i64,
    },
    /// B.cond, CBZ, TBZ ...: to `offset` or the next instruction.
    CondJump {
        /// Bytes from the instruction.
        offset: i64,
    },
    /// BL: a call to `offset`, then the next instruction.
    Call {
        /// Bytes from the instruction.
        offset: i64,
    },
    /// BLR (and BLRAA ...): a call through a register.
    CallReg,
    /// BR (and BRAA ...): a jump through a register (a jump table, or a tail call).
    JumpReg,
    /// RET (and RETAA ...).
    Return,
    /// SVC: a supervisor call, then the next instruction.
    Svc,
    /// BRK, HLT, UDF, HVC, SMC, DCPS, ERET, DRPS: the instruction traps (the last five are undefined at EL0).
    Trap,
}

impl Insn {
    /// What it does to control flow.
    pub fn flow(&self) -> Flow {
        match *self {
            Insn::B { link: false, offset } => Flow::Jump { offset },
            Insn::B { link: true, offset } => Flow::Call { offset },
            Insn::BCond { offset, .. } | Insn::Cbz { offset, .. } | Insn::Tbz { offset, .. } => {
                Flow::CondJump { offset }
            }
            Insn::BranchReg { op: BranchRegOp::Br, .. } => Flow::JumpReg,
            Insn::BranchReg { op: BranchRegOp::Blr, .. } => Flow::CallReg,
            Insn::BranchReg { op: BranchRegOp::Ret, .. } => Flow::Return,
            Insn::BranchReg { op: BranchRegOp::Eret | BranchRegOp::Drps, .. } => Flow::Trap,
            Insn::Svc { .. } => Flow::Svc,
            Insn::Brk { .. }
            | Insn::Hlt { .. }
            | Insn::Udf { .. }
            | Insn::Hvc { .. }
            | Insn::Smc { .. }
            | Insn::Dcps { .. } => Flow::Trap,
            _ => Flow::Next,
        }
    }

    /// Whether it may write X`x` (`x` 31: SP, where the form's register 31 is SP; a form's XZR is no write): a
    /// destination, a load's Rt (and a pair's Rt2), a store-exclusive's status, CAS's Rs, an address's writeback,
    /// and BL's and BLR's X30.
    pub fn writes(&self, x: u8) -> bool {
        let rd = |r: Reg| r.0 == x && x != 31; // (31 is XZR)
        let sp_or = |r: Reg| r.0 == x; // (31 is SP)
        let back = |a: &Addr| matches!(a, Addr::PreIndex { rn, .. } | Addr::PostIndex { rn, .. } if rn.0 == x);
        match self {
            Insn::Adr { rd: d, .. } => rd(*d),
            Insn::AddSubImm { setflags, rd: d, .. } | Insn::AddSubExt { setflags, rd: d, .. } => {
                if *setflags {
                    rd(*d)
                } else {
                    sp_or(*d)
                }
            }
            Insn::LogicalImm { op, rd: d, .. } => {
                if *op == LogicOp::Ands {
                    rd(*d)
                } else {
                    sp_or(*d)
                }
            }
            Insn::MoveWide { rd: d, .. }
            | Insn::Bitfield { rd: d, .. }
            | Insn::Extr { rd: d, .. }
            | Insn::LogicalReg { rd: d, .. }
            | Insn::AddSubReg { rd: d, .. }
            | Insn::AddSubCarry { rd: d, .. }
            | Insn::CondSelect { rd: d, .. }
            | Insn::DataProc1 { rd: d, .. }
            | Insn::DataProc2 { rd: d, .. }
            | Insn::Crc32 { rd: d, .. }
            | Insn::Pac { rd: d, .. }
            | Insn::Xpac { rd: d, .. }
            | Insn::DataProc3 { rd: d, .. }
            | Insn::FpToInt { rd: d, .. }
            | Insn::SimdMovToGen { rd: d, .. } => rd(*d),
            Insn::B { link: true, .. } | Insn::BranchReg { op: BranchRegOp::Blr, .. } => x == 30,
            Insn::Sys { l: true, rt, .. } | Insn::Mrs { rt, .. } => rd(*rt),
            Insn::LoadStore { kind, rt, addr, .. } => {
                back(addr) || (matches!(kind, LsKind::Load | LsKind::LoadSigned { .. }) && rd(*rt))
            }
            Insn::Pair { kind, rt, rt2, addr, .. } => {
                back(addr) || (matches!(kind, PairKind::Load | PairKind::LoadSignedWord) && (rd(*rt) || rd(*rt2)))
            }
            Insn::Exclusive { load, rs, rt, rt2, .. } => {
                if *load {
                    rd(*rt) || rt2.is_some_and(rd)
                } else {
                    rd(*rs)
                }
            }
            Insn::Ordered { load: true, rt, .. } | Insn::Ldapr { rt, .. } | Insn::Atomic { rt, .. } => rd(*rt),
            Insn::Cas { pair, rs, .. } => rd(*rs) || (*pair && x != 31 && rs.0 + 1 == x),
            Insn::LdrPac { writeback, rt, rn, .. } => rd(*rt) || (*writeback && rn.0 == x),
            Insn::FmovGen { to_general: true, gpr, .. } => rd(*gpr),
            Insn::SimdLdSt { rn, post: Some(_), .. } => rn.0 == x,
            _ => false,
        }
    }

    /// ADR's or ADRP's address (ADRP's: its page plus the offset), for the instruction at `pc`.
    pub fn adr_target(&self, pc: u64) -> Option<u64> {
        match *self {
            Insn::Adr { page: true, imm, .. } => Some((pc & !0xfff).wrapping_add_signed(imm)),
            Insn::Adr { page: false, imm, .. } => Some(pc.wrapping_add_signed(imm)),
            _ => None,
        }
    }
}
