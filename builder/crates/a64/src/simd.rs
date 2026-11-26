//! The operand types and operations of the floating-point and Advanced SIMD instructions.
//!
//! **Shapes are the encodings'.** An instruction's `size` and `Q` fields are kept as they are encoded (an [`Arr`], or a
//! [`Shape`] for the forms that also have a scalar encoding), and each operation's docs say what they mean for it: for
//! most, the elements of every operand; for a widening or narrowing one, the narrow operand's (the Arm ARM's `Ta`/`Tb`
//! are both worked out from it). The operations are named as their mnemonics, so `Simd3Op::Sqrdmulh` is SQRDMULH.

use crate::{Reg, Size};

/// A SIMD and FP register, V0 to V31 (as a scalar: B0, H0, S0, D0 or Q0, as the instruction's size says).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VReg(pub u8);

impl VReg {
    /// The register `n` after this one, modulo 32 (the next register of a list: V31 is followed by V0).
    pub fn plus(self, n: u8) -> VReg {
        VReg((self.0 + n) % 32)
    }
}

/// A floating-point format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FpType {
    /// Half precision (binary16).
    H,
    /// Single precision (binary32).
    S,
    /// Double precision (binary64).
    D,
}

impl FpType {
    /// Its size (`Size(1)` .. `Size(3)`).
    pub fn size(self) -> Size {
        match self {
            FpType::H => Size(1),
            FpType::S => Size(2),
            FpType::D => Size(3),
        }
    }

    /// Its width in bits.
    pub fn bits(self) -> u32 {
        self.size().bytes() * 8
    }

    /// The format of `size` (1 to 3).
    pub fn of_size(size: Size) -> Option<FpType> {
        match size.0 {
            1 => Some(FpType::H),
            2 => Some(FpType::S),
            3 => Some(FpType::D),
            _ => None,
        }
    }
}

/// A vector arrangement: its elements' size, and whether it fills the register (`q`) or its low 64 bits (8B, 16B, 4H,
/// 8H, 2S, 4S, 1D, 2D).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arr {
    /// The elements' size (`Size(0)` .. `Size(3)`).
    pub size: Size,
    /// The whole 128-bit register, else its low half.
    pub q: bool,
}

impl Arr {
    /// The arrangement of `size` elements, of 128 bits when `q`.
    pub fn new(size: Size, q: bool) -> Arr {
        Arr { size, q }
    }

    /// Its elements.
    pub fn lanes(self) -> u32 {
        (if self.q { 16 } else { 8 }) / self.size.bytes()
    }

    /// Its width in bits (64 or 128).
    pub fn bits(self) -> u32 {
        if self.q { 128 } else { 64 }
    }
}

/// The operands of a form that has both a vector and a scalar encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// A vector of this arrangement.
    Vector(Arr),
    /// A scalar of this size: one element, in the low bits of the register (the rest of it zeroed when written).
    Scalar(Size),
}

impl Shape {
    /// The elements' size.
    pub fn size(self) -> Size {
        match self {
            Shape::Vector(a) => a.size,
            Shape::Scalar(s) => s,
        }
    }

    /// How many elements it has.
    pub fn lanes(self) -> u32 {
        match self {
            Shape::Vector(a) => a.lanes(),
            Shape::Scalar(_) => 1,
        }
    }

    /// Whether it is the scalar form.
    pub fn is_scalar(self) -> bool {
        matches!(self, Shape::Scalar(_))
    }
}

/// A rounding, as the conversions and FRINT name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Round {
    /// To nearest, ties to even (N).
    Nearest,
    /// Towards plus infinity (P).
    Plus,
    /// Towards minus infinity (M).
    Minus,
    /// Towards zero (Z).
    Zero,
    /// To nearest, ties away from zero (A).
    Away,
    /// FPCR's mode, raising Inexact (FRINTX's X).
    Exact,
    /// FPCR's mode (FRINTI's I).
    Current,
}

impl Round {
    /// The letter of a mnemonic: n, p, m, z, a, x, i.
    pub fn letter(self) -> &'static str {
        match self {
            Round::Nearest => "n",
            Round::Plus => "p",
            Round::Minus => "m",
            Round::Zero => "z",
            Round::Away => "a",
            Round::Exact => "x",
            Round::Current => "i",
        }
    }
}

// ---- scalar floating point

/// The one-source scalar FP operations (FCVT between formats is [`crate::Insn::FpCvt`]).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fp1Op {
    Mov,
    Abs,
    Neg,
    Sqrt,
    /// FRINTN ... FRINTI.
    Rint(Round),
}

/// The two-source scalar FP operations.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fp2Op {
    Mul,
    Div,
    Add,
    Sub,
    Max,
    Min,
    MaxNm,
    MinNm,
    /// FNMUL: the product, negated.
    NMul,
}

/// The fused multiply-adds: FMADD (Ra + Rn × Rm), FMSUB (Ra − Rn × Rm), FNMADD (−Ra − Rn × Rm), FNMSUB (−Ra + Rn ×
/// Rm).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fp3Op {
    Madd,
    Msub,
    Nmadd,
    Nmsub,
}

/// Where FMOV (general) moves: which general register width, which part of the SIMD and FP register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FmovPart {
    /// Wn and Hn (the low 16 bits of the W register; FEAT_FP16).
    WH,
    /// Xn and Hn.
    XH,
    /// Wn and Sn.
    WS,
    /// Xn and Dn.
    XD,
    /// Xn and Vn.D\[1\], the register's upper half (the rest of it kept when written).
    XTop,
}

// ---- Advanced SIMD

/// The three-register operations whose operands are all of one shape (the Arm ARM's "three same" groups, those of FP16
/// and the "three same extra" group). For SDOT and UDOT the shape is the accumulator's (2S, 4S): their sources are
/// bytes, four to each of its elements. The FP ones' shape is of H, S or D elements.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Simd3Op {
    Shadd,
    Uhadd,
    Sqadd,
    Uqadd,
    Srhadd,
    Urhadd,
    And,
    Bic,
    Orr,
    Orn,
    Eor,
    Bsl,
    Bit,
    Bif,
    Shsub,
    Uhsub,
    Sqsub,
    Uqsub,
    Cmgt,
    Cmhi,
    Cmge,
    Cmhs,
    Sshl,
    Ushl,
    Sqshl,
    Uqshl,
    Srshl,
    Urshl,
    Sqrshl,
    Uqrshl,
    Smax,
    Umax,
    Smin,
    Umin,
    Sabd,
    Uabd,
    Saba,
    Uaba,
    Add,
    Sub,
    Cmtst,
    Cmeq,
    Mla,
    Mls,
    Mul,
    Pmul,
    Smaxp,
    Umaxp,
    Sminp,
    Uminp,
    Sqdmulh,
    Sqrdmulh,
    Addp,
    Sqrdmlah,
    Sqrdmlsh,
    Sdot,
    Udot,
    Fmaxnm,
    Fmla,
    Fadd,
    Fmulx,
    Fcmeq,
    Fmax,
    Frecps,
    Fminnm,
    Fmls,
    Fsub,
    Fmin,
    Frsqrts,
    Fmaxnmp,
    Faddp,
    Fmul,
    Fcmge,
    Facge,
    Fmaxp,
    Fdiv,
    Fminnmp,
    Fabd,
    Fcmgt,
    Facgt,
    Fminp,
}

impl Simd3Op {
    /// Whether it is a floating-point operation (FADD ... FMINP): its shape is of H, S or D floats.
    pub fn is_fp(self) -> bool {
        use Simd3Op::*;
        matches!(
            self,
            Fmaxnm
                | Fmla
                | Fadd
                | Fmulx
                | Fcmeq
                | Fmax
                | Frecps
                | Fminnm
                | Fmls
                | Fsub
                | Fmin
                | Frsqrts
                | Fmaxnmp
                | Faddp
                | Fmul
                | Fcmge
                | Facge
                | Fmaxp
                | Fdiv
                | Fminnmp
                | Fabd
                | Fcmgt
                | Facgt
                | Fminp
        )
    }
}

/// The three-register operations of mixed widths (the "three different" group). The instruction's shape is its narrow
/// operands' (the "2" forms, Q set, take the upper halves of the narrow sources, or write the upper half of a narrow
/// result): the long forms (…L) widen both sources, the wide forms (…W) the second, the narrowing ones (…HN) narrow
/// the result. PMULL of 1D elements makes one 128-bit product (1Q).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffOp {
    Saddl,
    Uaddl,
    Saddw,
    Uaddw,
    Ssubl,
    Usubl,
    Ssubw,
    Usubw,
    Addhn,
    Raddhn,
    Sabal,
    Uabal,
    Subhn,
    Rsubhn,
    Sabdl,
    Uabdl,
    Smlal,
    Umlal,
    Sqdmlal,
    Smlsl,
    Umlsl,
    Sqdmlsl,
    Smull,
    Umull,
    Sqdmull,
    Pmull,
}

impl DiffOp {
    /// Whether its result is narrow (ADDHN, RADDHN, SUBHN, RSUBHN), its first source wide (the …W forms), else both
    /// sources narrow and the result wide.
    pub fn form(self) -> DiffForm {
        match self {
            DiffOp::Addhn | DiffOp::Raddhn | DiffOp::Subhn | DiffOp::Rsubhn => DiffForm::Narrow,
            DiffOp::Saddw | DiffOp::Uaddw | DiffOp::Ssubw | DiffOp::Usubw => DiffForm::Wide,
            _ => DiffForm::Long,
        }
    }
}

/// Which operands of a [`DiffOp`] are wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffForm {
    /// Narrow sources, a wide result.
    Long,
    /// A wide first source and result, a narrow second source.
    Wide,
    /// Wide sources, a narrow result.
    Narrow,
}

/// A comparison with zero (CMxx #0, FCMxx #0.0).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmpZero {
    Gt,
    Ge,
    Eq,
    Le,
    Lt,
}

impl CmpZero {
    /// gt, ge, eq, le, lt.
    pub fn name(self) -> &'static str {
        match self {
            CmpZero::Gt => "gt",
            CmpZero::Ge => "ge",
            CmpZero::Eq => "eq",
            CmpZero::Le => "le",
            CmpZero::Lt => "lt",
        }
    }
}

/// The two-register operations (the "two-register miscellaneous" groups, vector, scalar and FP16). The shape is the
/// encoding's: for the narrowing ones (XTN, SQXTN, UQXTN, SQXTUN, FCVTN, FCVTXN) the result's, for the widening ones
/// (SADDLP, UADDLP, SADALP, UADALP, SHLL, FCVTL) the source's. FCVTN, FCVTXN and FCVTL's shape has the narrow format's
/// size (H or S).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Simd2Op {
    Rev64,
    Rev32,
    Rev16,
    Saddlp,
    Uaddlp,
    Suqadd,
    Usqadd,
    Cls,
    Clz,
    Cnt,
    Not,
    Rbit,
    Sadalp,
    Uadalp,
    Sqabs,
    Sqneg,
    /// CMGT, CMGE, CMEQ, CMLE, CMLT with zero.
    Cmp0(CmpZero),
    Abs,
    Neg,
    Xtn,
    Sqxtn,
    Uqxtn,
    Sqxtun,
    /// SHLL: each element widened and shifted left by its width.
    Shll,
    Fcvtn,
    Fcvtxn,
    Fcvtl,
    /// FRINTN ... FRINTI.
    Frint(Round),
    /// FCVTNS ... FCVTZU (`Round::Nearest`, `Plus`, `Minus`, `Zero` or `Away`).
    FcvtInt {
        round: Round,
        unsigned: bool,
    },
    Scvtf,
    Ucvtf,
    Urecpe,
    Ursqrte,
    Frecpe,
    Frsqrte,
    Frecpx,
    /// FCMGT, FCMGE, FCMEQ, FCMLE, FCMLT with zero.
    Fcmp0(CmpZero),
    Fabs,
    Fneg,
    Fsqrt,
}

/// The operations across the lanes of a vector, to a scalar.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AcrossOp {
    Saddlv,
    Uaddlv,
    Smaxv,
    Umaxv,
    Sminv,
    Uminv,
    Addv,
    Fmaxnmv,
    Fminnmv,
    Fmaxv,
    Fminv,
}

/// The scalar pairwise operations: the two elements of a vector (2S, 2D, or 2H for FP16) to a scalar.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PairwiseOp {
    Addp,
    Fmaxnmp,
    Faddp,
    Fmaxp,
    Fminnmp,
    Fminp,
}

/// The shifts by an immediate. The shape is the source's elements' (a narrowing shift's source is wide, a widening
/// one's narrow); `shift` is the right shift (1 to the element's width) or the left one (0 to its width less one), as
/// the mnemonic shifts; for the fixed-point conversions it is the fraction bits.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShiftOp {
    Sshr,
    Ushr,
    Ssra,
    Usra,
    Srshr,
    Urshr,
    Srsra,
    Ursra,
    Sri,
    Shl,
    Sli,
    Sqshlu,
    Sqshl,
    Uqshl,
    Shrn,
    Rshrn,
    Sqshrun,
    Sqrshrun,
    Sqshrn,
    Uqshrn,
    Sqrshrn,
    Uqrshrn,
    Sshll,
    Ushll,
    Scvtf,
    Ucvtf,
    Fcvtzs,
    Fcvtzu,
}

impl ShiftOp {
    /// Whether it shifts right (its `shift` is 1 to the width), else left (0 to the width less one).
    pub fn right(self) -> bool {
        !matches!(
            self,
            ShiftOp::Shl
                | ShiftOp::Sli
                | ShiftOp::Sqshlu
                | ShiftOp::Sqshl
                | ShiftOp::Uqshl
                | ShiftOp::Sshll
                | ShiftOp::Ushll
        )
    }

    /// Whether its result is half the width of its source.
    pub fn narrows(self) -> bool {
        matches!(
            self,
            ShiftOp::Shrn
                | ShiftOp::Rshrn
                | ShiftOp::Sqshrun
                | ShiftOp::Sqrshrun
                | ShiftOp::Sqshrn
                | ShiftOp::Uqshrn
                | ShiftOp::Sqrshrn
                | ShiftOp::Uqrshrn
        )
    }

    /// Whether its result is twice the width of its source.
    pub fn widens(self) -> bool {
        matches!(self, ShiftOp::Sshll | ShiftOp::Ushll)
    }
}

/// The operations with an element of a register (the "x indexed element" groups). The shape is the first source's
/// (the narrow one of a widening form: SMLAL, SQDMULL ... take the upper half with Q, the "2" forms). For SDOT and UDOT
/// it is the accumulator's (2S, 4S) and the element is a group of four bytes (`.4b[i]`).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ElemOp {
    Fmla,
    Fmls,
    Fmul,
    Fmulx,
    Mla,
    Mls,
    Mul,
    Smlal,
    Umlal,
    Smlsl,
    Umlsl,
    Smull,
    Umull,
    Sqdmlal,
    Sqdmlsl,
    Sqdmull,
    Sqdmulh,
    Sqrdmulh,
    Sqrdmlah,
    Sqrdmlsh,
    Sdot,
    Udot,
}

impl ElemOp {
    /// Whether its result is twice its sources' width.
    pub fn widens(self) -> bool {
        matches!(
            self,
            ElemOp::Smlal
                | ElemOp::Umlal
                | ElemOp::Smlsl
                | ElemOp::Umlsl
                | ElemOp::Smull
                | ElemOp::Umull
                | ElemOp::Sqdmlal
                | ElemOp::Sqdmlsl
                | ElemOp::Sqdmull
        )
    }
}

/// UZP, TRN, ZIP (the …1 and …2 forms: [`crate::Insn::SimdPermute`]'s `second`).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermuteOp {
    Uzp,
    Trn,
    Zip,
}

/// What a modified-immediate instruction does with its value.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModImmOp {
    Movi,
    Mvni,
    Orr,
    Bic,
    /// FMOV (vector, immediate): `imm8` is an FP8 immediate of the elements' format.
    Fmov,
}

/// How a modified immediate's 8 bits make an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModShift {
    /// Shifted left by `amount` (0, 8, 16 or 24), zeros shifted in.
    Lsl(u8),
    /// Shifted left by `amount` (8 or 16), ones shifted in (MSL).
    Msl(u8),
    /// Each bit of `imm8` a byte of ones or zeros (MOVI of 64-bit elements).
    Bytes,
    /// An FP8 immediate expanded to the elements' format (FMOV).
    Float,
}

/// A modified immediate: the operation, the arrangement, the 8 bits and how they make each element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModImm {
    /// What it does.
    pub op: ModImmOp,
    /// The elements it writes (1D for the scalar MOVI Dd).
    pub arr: Arr,
    /// The encoded 8 bits (abcdefgh).
    pub imm8: u8,
    /// How they make each element.
    pub shift: ModShift,
}

impl ModImm {
    /// The element's value (in its low `arr.size` bytes).
    pub fn element(self) -> u64 {
        let imm = u64::from(self.imm8);
        match self.shift {
            ModShift::Lsl(n) => imm << n,
            ModShift::Msl(n) => (imm << n) | ((1u64 << n) - 1),
            ModShift::Bytes => (0..8).fold(0, |acc, i| if imm >> i & 1 == 1 { acc | 0xff << (8 * i) } else { acc }),
            ModShift::Float => {
                fp_expand_imm(self.imm8, FpType::of_size(self.arr.size).expect("an FMOV of H, S or D elements"))
            }
        }
    }
}

/// VFPExpandImm: the FP8 immediate `imm8` (sign, 3 exponent bits, 4 fraction bits) as a value of `ty`, its bits.
pub fn fp_expand_imm(imm8: u8, ty: FpType) -> u64 {
    let (e, f) = match ty {
        FpType::H => (5, 10),
        FpType::S => (8, 23),
        FpType::D => (11, 52),
    };
    let sign = u64::from(imm8 >> 7);
    let b6 = u64::from(imm8 >> 6 & 1);
    let exp = ((1 - b6) << (e - 1)) | (if b6 == 1 { (1u64 << (e - 3)) - 1 } else { 0 }) << 2 | u64::from(imm8 >> 4 & 3);
    let frac = u64::from(imm8 & 15) << (f - 4);
    (sign << (e + f)) | (exp << f) | frac
}

/// The elements a SIMD structure load or store moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Structure {
    /// LD1 ... ST4 (multiple structures): `regs` registers of `arr`, interleaved by `selem` (LD1 and ST1 of one to four
    /// registers have `selem` 1; LD2/ST2 two registers, interleaved in pairs; and so on).
    Multiple {
        /// The registers' arrangement.
        arr: Arr,
        /// How many registers.
        regs: u8,
        /// How many elements each structure has (1 to 4).
        selem: u8,
    },
    /// LD1 ... ST4 (single structure): element `index` of `selem` consecutive registers.
    Lane {
        /// The elements' size.
        size: Size,
        /// Which element.
        index: u8,
        /// How many registers.
        selem: u8,
    },
    /// LD1R ... LD4R: one structure, each of its `selem` elements replicated across a register of `arr`.
    Replicate {
        /// The registers' arrangement.
        arr: Arr,
        /// How many registers.
        selem: u8,
    },
}

impl Structure {
    /// The bytes it moves.
    pub fn bytes(self) -> u32 {
        match self {
            Structure::Multiple { arr, regs, .. } => u32::from(regs) * arr.bits() / 8,
            Structure::Lane { size, selem, .. } => u32::from(selem) * size.bytes(),
            Structure::Replicate { arr, selem } => u32::from(selem) * arr.size.bytes(),
        }
    }

    /// The registers it names.
    pub fn regs(self) -> u8 {
        match self {
            Structure::Multiple { regs, .. } => regs,
            Structure::Lane { selem, .. } | Structure::Replicate { selem, .. } => selem,
        }
    }
}

/// A structure access's post-index: by the bytes it moves (Rm 31, written as that immediate), or by a register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostIndex {
    /// Rn += the bytes moved.
    Imm(u32),
    /// Rn += Xm.
    Reg(Reg),
}

/// The AES instructions.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AesOp {
    Aese,
    Aesd,
    Aesmc,
    Aesimc,
}

/// The SHA instructions of three registers.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sha3Op {
    Sha1c,
    Sha1p,
    Sha1m,
    Sha1su0,
    Sha256h,
    Sha256h2,
    Sha256su1,
}

/// The SHA instructions of two registers.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sha2Op {
    Sha1h,
    Sha1su1,
    Sha256su0,
}
