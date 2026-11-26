//! The floating-point and Advanced SIMD instructions as llvm-mc writes them with `-M no-aliases`.

use super::{r, x, xsp};
use crate::*;
use std::fmt::{self, Display, Formatter};

const SCALAR: [&str; 5] = ["b", "h", "s", "d", "q"];

/// A register as a scalar of `size`: b0, h0, s0, d0, q0.
struct Scalar(Size, VReg);

impl Display for Scalar {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", SCALAR[usize::from(self.0.0)], self.1.0)
    }
}

impl Display for Arr {
    /// 8b, 16b, 4h, 8h, 2s, 4s, 1d, 2d.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.lanes(), SCALAR[usize::from(self.size.0)])
    }
}

/// A register as a vector: v0.4s.
struct Vector(Arr, VReg);

impl Display for Vector {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}", self.1.0, self.0)
    }
}

/// An element of a register: v2.s[3].
struct Element(Size, VReg, u8);

impl Display for Element {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}[{}]", self.1.0, SCALAR[usize::from(self.0.0)], self.2)
    }
}

/// A register as an operand of `shape`.
struct Operand(Shape, VReg);

impl Display for Operand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Shape::Vector(a) => Vector(a, self.1).fmt(f),
            Shape::Scalar(s) => Scalar(s, self.1).fmt(f),
        }
    }
}

/// `shape` with elements twice as wide: a vector's fills the register (8b, 16b → 8h; 1d → 1q).
fn wide(shape: Shape) -> Shape {
    let size = Size(shape.size().0 + 1);
    match shape {
        Shape::Vector(_) => Shape::Vector(Arr::new(size, true)),
        Shape::Scalar(_) => Shape::Scalar(size),
    }
}

/// The "2" of a vector form that reads or writes a register's upper half.
fn two(shape: Shape) -> &'static str {
    match shape {
        Shape::Vector(Arr { q: true, .. }) => "2",
        _ => "",
    }
}

/// A float as llvm-mc writes an FP immediate: `#1.00000000`.
struct FpImm(u8);

impl Display for FpImm {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "#{:.8}", f64::from_bits(fp_expand_imm(self.0, FpType::D)))
    }
}

/// A 64-bit immediate as llvm-mc writes MOVI's (C's `%#016llx`: 0 has no prefix).
struct Hex64(u64);

impl Display for Hex64 {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.0 == 0 { f.write_str("0000000000000000") } else { write!(f, "{:#016x}", self.0) }
    }
}

/// A register list: `{ v0.4s, v1.4s }` (`what` the suffix of each).
struct List<'a>(VReg, u8, &'a dyn Display);

impl Display for List<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("{ ")?;
        for i in 0..self.1 {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "v{}.{}", self.0.plus(i).0, self.2)?;
        }
        f.write_str(" }")
    }
}

impl FpType {
    fn reg(self, v: VReg) -> Scalar {
        Scalar(self.size(), v)
    }
}

/// Writes one of the floating-point or Advanced SIMD instructions.
pub(super) fn fmt(f: &mut Formatter<'_>, insn: &Insn) -> fmt::Result {
    match *insn {
        Insn::Fp1 { op, ty, rd, rn } => {
            let name = match op {
                Fp1Op::Mov => "fmov".to_string(),
                Fp1Op::Abs => "fabs".to_string(),
                Fp1Op::Neg => "fneg".to_string(),
                Fp1Op::Sqrt => "fsqrt".to_string(),
                Fp1Op::Rint(r) => format!("frint{}", r.letter()),
            };
            write!(f, "{name} {}, {}", ty.reg(rd), ty.reg(rn))
        }
        Insn::FpCvt { from, to, rd, rn } => write!(f, "fcvt {}, {}", to.reg(rd), from.reg(rn)),
        Insn::Fp2 { op, ty, rd, rn, rm } => {
            let name = match op {
                Fp2Op::Mul => "fmul",
                Fp2Op::Div => "fdiv",
                Fp2Op::Add => "fadd",
                Fp2Op::Sub => "fsub",
                Fp2Op::Max => "fmax",
                Fp2Op::Min => "fmin",
                Fp2Op::MaxNm => "fmaxnm",
                Fp2Op::MinNm => "fminnm",
                Fp2Op::NMul => "fnmul",
            };
            write!(f, "{name} {}, {}, {}", ty.reg(rd), ty.reg(rn), ty.reg(rm))
        }
        Insn::Fp3 { op, ty, rd, rn, rm, ra } => {
            let name = match op {
                Fp3Op::Madd => "fmadd",
                Fp3Op::Msub => "fmsub",
                Fp3Op::Nmadd => "fnmadd",
                Fp3Op::Nmsub => "fnmsub",
            };
            write!(f, "{name} {}, {}, {}, {}", ty.reg(rd), ty.reg(rn), ty.reg(rm), ty.reg(ra))
        }
        Insn::FpCmp { ty, signaling, rn, rm } => {
            write!(f, "fcmp{} {}, ", if signaling { "e" } else { "" }, ty.reg(rn))?;
            match rm {
                Some(rm) => write!(f, "{}", ty.reg(rm)),
                None => f.write_str("#0.0"),
            }
        }
        Insn::FpCondCmp { ty, signaling, rn, rm, nzcv, cond } => write!(
            f,
            "fccmp{} {}, {}, #{nzcv}, {}",
            if signaling { "e" } else { "" },
            ty.reg(rn),
            ty.reg(rm),
            cond.name()
        ),
        Insn::FpCsel { ty, rd, rn, rm, cond } => {
            write!(f, "fcsel {}, {}, {}, {}", ty.reg(rd), ty.reg(rn), ty.reg(rm), cond.name())
        }
        Insn::FpImm { ty, rd, imm8 } => write!(f, "fmov {}, {}", ty.reg(rd), FpImm(imm8)),
        Insn::FpToInt { ty, sf, round, unsigned, fbits, rd, rn } => {
            write!(f, "fcvt{}{} {}, {}", round.letter(), if unsigned { "u" } else { "s" }, r(sf, rd), ty.reg(rn))?;
            if fbits != 0 { write!(f, ", #{fbits}") } else { Ok(()) }
        }
        Insn::IntToFp { ty, sf, unsigned, fbits, rd, rn } => {
            write!(f, "{}cvtf {}, {}", if unsigned { "u" } else { "s" }, ty.reg(rd), r(sf, rn))?;
            if fbits != 0 { write!(f, ", #{fbits}") } else { Ok(()) }
        }
        Insn::FmovGen { part, to_general, gpr, fpr } => {
            let (g, v) = match part {
                FmovPart::WH => (r(false, gpr).to_string(), Scalar(Size(1), fpr).to_string()),
                FmovPart::XH => (x(gpr).to_string(), Scalar(Size(1), fpr).to_string()),
                FmovPart::WS => (r(false, gpr).to_string(), Scalar(Size(2), fpr).to_string()),
                FmovPart::XD => (x(gpr).to_string(), Scalar(Size(3), fpr).to_string()),
                FmovPart::XTop => (x(gpr).to_string(), Element(Size(3), fpr, 1).to_string()),
            };
            if to_general { write!(f, "fmov {g}, {v}") } else { write!(f, "fmov {v}, {g}") }
        }
        Insn::Simd3 { op, shape, rd, rn, rm } => match op {
            Simd3Op::Sdot | Simd3Op::Udot => {
                let q = matches!(shape, Shape::Vector(Arr { q: true, .. }));
                let src = Arr::new(Size(0), q);
                write!(f, "{} {}, {}, {}", op.name(), Operand(shape, rd), Vector(src, rn), Vector(src, rm))
            }
            _ => write!(f, "{} {}, {}, {}", op.name(), Operand(shape, rd), Operand(shape, rn), Operand(shape, rm)),
        },
        Insn::SimdDiff { op, shape, rd, rn, rm } => {
            let (d, n, m) = match op.form() {
                DiffForm::Long => (wide(shape), shape, shape),
                DiffForm::Wide => (wide(shape), wide(shape), shape),
                DiffForm::Narrow => (shape, wide(shape), wide(shape)),
            };
            let (d, n, m) = (Operand(d, rd), Operand(n, rn), Operand(m, rm));
            write!(f, "{}{} {d}, {n}, {m}", op.name(), two(shape))
        }
        Insn::Simd2 { op, shape, rd, rn } => simd2(f, op, shape, rd, rn),
        Insn::SimdAcross { op, arr, rd, rn } => {
            let size = if matches!(op, AcrossOp::Saddlv | AcrossOp::Uaddlv) { Size(arr.size.0 + 1) } else { arr.size };
            write!(f, "{} {}, {}", op.name(), Scalar(size, rd), Vector(arr, rn))
        }
        Insn::SimdPairwise { op, size, rd, rn } => {
            write!(f, "{} {}, v{}.2{}", op.name(), Scalar(size, rd), rn.0, SCALAR[usize::from(size.0)])
        }
        Insn::SimdShift { op, shape, rd, rn, shift } => {
            let (d, n, suffix) = if op.narrows() {
                (shape, wide(shape), two(shape))
            } else if op.widens() {
                (wide(shape), shape, two(shape))
            } else {
                (shape, shape, "")
            };
            write!(f, "{}{suffix} {}, {}, #{shift}", op.name(), Operand(d, rd), Operand(n, rn))
        }
        Insn::SimdElem { op, shape, rd, rn, rm, index } => {
            if matches!(op, ElemOp::Sdot | ElemOp::Udot) {
                let q = matches!(shape, Shape::Vector(Arr { q: true, .. }));
                let src = Vector(Arr::new(Size(0), q), rn);
                return write!(f, "{} {}, {src}, v{}.4b[{index}]", op.name(), Operand(shape, rd), rm.0);
            }
            let (d, suffix) = if op.widens() { (wide(shape), two(shape)) } else { (shape, "") };
            let e = Element(shape.size(), rm, index);
            write!(f, "{}{suffix} {}, {}, {e}", op.name(), Operand(d, rd), Operand(shape, rn))
        }
        Insn::SimdPermute { op, second, arr, rd, rn, rm } => {
            let name = match op {
                PermuteOp::Uzp => "uzp",
                PermuteOp::Trn => "trn",
                PermuteOp::Zip => "zip",
            };
            let n = if second { 2 } else { 1 };
            write!(f, "{name}{n} {}, {}, {}", Vector(arr, rd), Vector(arr, rn), Vector(arr, rm))
        }
        Insn::SimdExt { q, rd, rn, rm, index } => {
            let a = Arr::new(Size(0), q);
            write!(f, "ext {}, {}, {}, #{index}", Vector(a, rd), Vector(a, rn), Vector(a, rm))
        }
        Insn::SimdTable { q, tbx, regs, rd, rn, rm } => {
            let a = Arr::new(Size(0), q);
            let list = List(rn, regs, &"16b");
            write!(f, "{} {}, {list}, {}", if tbx { "tbx" } else { "tbl" }, Vector(a, rd), Vector(a, rm))
        }
        Insn::SimdDupElem { shape: Shape::Scalar(size), rd, rn, index } => {
            write!(f, "mov {}, {}", Scalar(size, rd), Element(size, rn, index))
        }
        Insn::SimdDupElem { shape: Shape::Vector(arr), rd, rn, index } => {
            write!(f, "dup {}, {}", Vector(arr, rd), Element(arr.size, rn, index))
        }
        Insn::SimdDupGen { arr, rd, rn } => write!(f, "dup {}, {}", Vector(arr, rd), r(arr.size.0 == 3, rn)),
        Insn::SimdInsElem { size, rd, dst, rn, src } => {
            write!(f, "ins {}, {}", Element(size, rd, dst), Element(size, rn, src))
        }
        Insn::SimdInsGen { size, rd, index, rn } => {
            write!(f, "ins {}, {}", Element(size, rd, index), r(size.0 == 3, rn))
        }
        Insn::SimdMovToGen { signed, x64, size, rd, rn, index } => {
            write!(f, "{} {}, {}", if signed { "smov" } else { "umov" }, r(x64, rd), Element(size, rn, index))
        }
        Insn::SimdModImm { imm, rd } => mod_imm(f, imm, rd),
        Insn::Aes { op, rd, rn } => {
            let name = match op {
                AesOp::Aese => "aese",
                AesOp::Aesd => "aesd",
                AesOp::Aesmc => "aesmc",
                AesOp::Aesimc => "aesimc",
            };
            let a = Arr::new(Size(0), true);
            write!(f, "{name} {}, {}", Vector(a, rd), Vector(a, rn))
        }
        Insn::Sha3 { op, rd, rn, rm } => {
            let s4 = Arr::new(Size(2), true);
            let m = Vector(s4, rm);
            match op {
                Sha3Op::Sha1c | Sha3Op::Sha1p | Sha3Op::Sha1m => {
                    write!(f, "{} {}, {}, {m}", op.name(), Scalar(Size(4), rd), Scalar(Size(2), rn))
                }
                Sha3Op::Sha256h | Sha3Op::Sha256h2 => {
                    write!(f, "{} {}, {}, {m}", op.name(), Scalar(Size(4), rd), Scalar(Size(4), rn))
                }
                Sha3Op::Sha1su0 | Sha3Op::Sha256su1 => {
                    write!(f, "{} {}, {}, {m}", op.name(), Vector(s4, rd), Vector(s4, rn))
                }
            }
        }
        Insn::Sha2 { op: Sha2Op::Sha1h, rd, rn } => {
            write!(f, "sha1h {}, {}", Scalar(Size(2), rd), Scalar(Size(2), rn))
        }
        Insn::Sha2 { op, rd, rn } => {
            let s4 = Arr::new(Size(2), true);
            let name = if op == Sha2Op::Sha1su1 { "sha1su1" } else { "sha256su0" };
            write!(f, "{name} {}, {}", Vector(s4, rd), Vector(s4, rn))
        }
        Insn::SimdLdSt { load, what, rt, rn, post } => {
            let verb = if load { "ld" } else { "st" };
            match what {
                Structure::Multiple { arr, regs, selem } => {
                    write!(f, "{verb}{selem} {}", List(rt, regs, &arr))?;
                }
                Structure::Lane { size, index, selem } => {
                    write!(f, "{verb}{selem} {}[{index}]", List(rt, selem, &SCALAR[usize::from(size.0)]))?;
                }
                Structure::Replicate { arr, selem } => write!(f, "{verb}{selem}r {}", List(rt, selem, &arr))?,
            }
            write!(f, ", [{}]", xsp(rn))?;
            match post {
                None => Ok(()),
                Some(PostIndex::Imm(n)) => write!(f, ", #{n}"),
                Some(PostIndex::Reg(m)) => write!(f, ", {}", x(m)),
            }
        }
        _ => unreachable!("not a floating-point or Advanced SIMD instruction: {insn:?}"),
    }
}

fn simd2(f: &mut Formatter<'_>, op: Simd2Op, shape: Shape, rd: VReg, rn: VReg) -> fmt::Result {
    use Simd2Op::*;
    let name = op.name();
    match op {
        Cmp0(_) => write!(f, "{name} {}, {}, #0", Operand(shape, rd), Operand(shape, rn)),
        Fcmp0(_) => write!(f, "{name} {}, {}, #0.0", Operand(shape, rd), Operand(shape, rn)),
        Xtn | Sqxtn | Uqxtn | Sqxtun | Fcvtn | Fcvtxn => {
            write!(f, "{name}{} {}, {}", two(shape), Operand(shape, rd), Operand(wide(shape), rn))
        }
        Fcvtl => write!(f, "{name}{} {}, {}", two(shape), Operand(wide(shape), rd), Operand(shape, rn)),
        Shll => write!(
            f,
            "{name}{} {}, {}, #{}",
            two(shape),
            Operand(wide(shape), rd),
            Operand(shape, rn),
            shape.size().bytes() * 8
        ),
        Saddlp | Uaddlp | Sadalp | Uadalp => {
            let Shape::Vector(a) = shape else { unreachable!("a vector form") };
            let d = Arr::new(Size(a.size.0 + 1), a.q);
            write!(f, "{name} {}, {}", Vector(d, rd), Vector(a, rn))
        }
        _ => write!(f, "{name} {}, {}", Operand(shape, rd), Operand(shape, rn)),
    }
}

fn mod_imm(f: &mut Formatter<'_>, imm: ModImm, rd: VReg) -> fmt::Result {
    let name = match imm.op {
        ModImmOp::Movi => "movi",
        ModImmOp::Mvni => "mvni",
        ModImmOp::Orr => "orr",
        ModImmOp::Bic => "bic",
        ModImmOp::Fmov => "fmov",
    };
    let d = if imm.arr.size.0 == 3 && !imm.arr.q && imm.op == ModImmOp::Movi {
        Scalar(Size(3), rd).to_string()
    } else {
        Vector(imm.arr, rd).to_string()
    };
    match imm.shift {
        ModShift::Lsl(0) => write!(f, "{name} {d}, #{}", imm.imm8),
        ModShift::Lsl(n) => write!(f, "{name} {d}, #{}, lsl #{n}", imm.imm8),
        ModShift::Msl(n) => write!(f, "{name} {d}, #{}, msl #{n}", imm.imm8),
        ModShift::Bytes => write!(f, "{name} {d}, #{}", Hex64(imm.element())),
        ModShift::Float => write!(f, "{name} {d}, {}", FpImm(imm.imm8)),
    }
}

impl Simd3Op {
    /// The mnemonic.
    pub fn name(self) -> &'static str {
        use Simd3Op::*;
        match self {
            Shadd => "shadd",
            Uhadd => "uhadd",
            Sqadd => "sqadd",
            Uqadd => "uqadd",
            Srhadd => "srhadd",
            Urhadd => "urhadd",
            And => "and",
            Bic => "bic",
            Orr => "orr",
            Orn => "orn",
            Eor => "eor",
            Bsl => "bsl",
            Bit => "bit",
            Bif => "bif",
            Shsub => "shsub",
            Uhsub => "uhsub",
            Sqsub => "sqsub",
            Uqsub => "uqsub",
            Cmgt => "cmgt",
            Cmhi => "cmhi",
            Cmge => "cmge",
            Cmhs => "cmhs",
            Sshl => "sshl",
            Ushl => "ushl",
            Sqshl => "sqshl",
            Uqshl => "uqshl",
            Srshl => "srshl",
            Urshl => "urshl",
            Sqrshl => "sqrshl",
            Uqrshl => "uqrshl",
            Smax => "smax",
            Umax => "umax",
            Smin => "smin",
            Umin => "umin",
            Sabd => "sabd",
            Uabd => "uabd",
            Saba => "saba",
            Uaba => "uaba",
            Add => "add",
            Sub => "sub",
            Cmtst => "cmtst",
            Cmeq => "cmeq",
            Mla => "mla",
            Mls => "mls",
            Mul => "mul",
            Pmul => "pmul",
            Smaxp => "smaxp",
            Umaxp => "umaxp",
            Sminp => "sminp",
            Uminp => "uminp",
            Sqdmulh => "sqdmulh",
            Sqrdmulh => "sqrdmulh",
            Addp => "addp",
            Sqrdmlah => "sqrdmlah",
            Sqrdmlsh => "sqrdmlsh",
            Sdot => "sdot",
            Udot => "udot",
            Fmaxnm => "fmaxnm",
            Fmla => "fmla",
            Fadd => "fadd",
            Fmulx => "fmulx",
            Fcmeq => "fcmeq",
            Fmax => "fmax",
            Frecps => "frecps",
            Fminnm => "fminnm",
            Fmls => "fmls",
            Fsub => "fsub",
            Fmin => "fmin",
            Frsqrts => "frsqrts",
            Fmaxnmp => "fmaxnmp",
            Faddp => "faddp",
            Fmul => "fmul",
            Fcmge => "fcmge",
            Facge => "facge",
            Fmaxp => "fmaxp",
            Fdiv => "fdiv",
            Fminnmp => "fminnmp",
            Fabd => "fabd",
            Fcmgt => "fcmgt",
            Facgt => "facgt",
            Fminp => "fminp",
        }
    }
}

impl DiffOp {
    /// The mnemonic (without the "2" of the upper-half forms).
    pub fn name(self) -> &'static str {
        use DiffOp::*;
        match self {
            Saddl => "saddl",
            Uaddl => "uaddl",
            Saddw => "saddw",
            Uaddw => "uaddw",
            Ssubl => "ssubl",
            Usubl => "usubl",
            Ssubw => "ssubw",
            Usubw => "usubw",
            Addhn => "addhn",
            Raddhn => "raddhn",
            Sabal => "sabal",
            Uabal => "uabal",
            Subhn => "subhn",
            Rsubhn => "rsubhn",
            Sabdl => "sabdl",
            Uabdl => "uabdl",
            Smlal => "smlal",
            Umlal => "umlal",
            Sqdmlal => "sqdmlal",
            Smlsl => "smlsl",
            Umlsl => "umlsl",
            Sqdmlsl => "sqdmlsl",
            Smull => "smull",
            Umull => "umull",
            Sqdmull => "sqdmull",
            Pmull => "pmull",
        }
    }
}

impl Simd2Op {
    /// The mnemonic (without the "2" of the upper-half forms).
    pub fn name(self) -> String {
        use Simd2Op::*;
        let s = match self {
            Rev64 => "rev64",
            Rev32 => "rev32",
            Rev16 => "rev16",
            Saddlp => "saddlp",
            Uaddlp => "uaddlp",
            Suqadd => "suqadd",
            Usqadd => "usqadd",
            Cls => "cls",
            Clz => "clz",
            Cnt => "cnt",
            Not => "not",
            Rbit => "rbit",
            Sadalp => "sadalp",
            Uadalp => "uadalp",
            Sqabs => "sqabs",
            Sqneg => "sqneg",
            Cmp0(c) => return format!("cm{}", c.name()),
            Abs => "abs",
            Neg => "neg",
            Xtn => "xtn",
            Sqxtn => "sqxtn",
            Uqxtn => "uqxtn",
            Sqxtun => "sqxtun",
            Shll => "shll",
            Fcvtn => "fcvtn",
            Fcvtxn => "fcvtxn",
            Fcvtl => "fcvtl",
            Frint(r) => return format!("frint{}", r.letter()),
            FcvtInt { round, unsigned } => {
                return format!("fcvt{}{}", round.letter(), if unsigned { "u" } else { "s" });
            }
            Scvtf => "scvtf",
            Ucvtf => "ucvtf",
            Urecpe => "urecpe",
            Ursqrte => "ursqrte",
            Frecpe => "frecpe",
            Frsqrte => "frsqrte",
            Frecpx => "frecpx",
            Fcmp0(c) => return format!("fcm{}", c.name()),
            Fabs => "fabs",
            Fneg => "fneg",
            Fsqrt => "fsqrt",
        };
        s.to_string()
    }
}

impl AcrossOp {
    /// The mnemonic.
    pub fn name(self) -> &'static str {
        use AcrossOp::*;
        match self {
            Saddlv => "saddlv",
            Uaddlv => "uaddlv",
            Smaxv => "smaxv",
            Umaxv => "umaxv",
            Sminv => "sminv",
            Uminv => "uminv",
            Addv => "addv",
            Fmaxnmv => "fmaxnmv",
            Fminnmv => "fminnmv",
            Fmaxv => "fmaxv",
            Fminv => "fminv",
        }
    }
}

impl PairwiseOp {
    /// The mnemonic.
    pub fn name(self) -> &'static str {
        use PairwiseOp::*;
        match self {
            Addp => "addp",
            Fmaxnmp => "fmaxnmp",
            Faddp => "faddp",
            Fmaxp => "fmaxp",
            Fminnmp => "fminnmp",
            Fminp => "fminp",
        }
    }
}

impl ShiftOp {
    /// The mnemonic (without the "2" of the upper-half forms).
    pub fn name(self) -> &'static str {
        use ShiftOp::*;
        match self {
            Sshr => "sshr",
            Ushr => "ushr",
            Ssra => "ssra",
            Usra => "usra",
            Srshr => "srshr",
            Urshr => "urshr",
            Srsra => "srsra",
            Ursra => "ursra",
            Sri => "sri",
            Shl => "shl",
            Sli => "sli",
            Sqshlu => "sqshlu",
            Sqshl => "sqshl",
            Uqshl => "uqshl",
            Shrn => "shrn",
            Rshrn => "rshrn",
            Sqshrun => "sqshrun",
            Sqrshrun => "sqrshrun",
            Sqshrn => "sqshrn",
            Uqshrn => "uqshrn",
            Sqrshrn => "sqrshrn",
            Uqrshrn => "uqrshrn",
            Sshll => "sshll",
            Ushll => "ushll",
            Scvtf => "scvtf",
            Ucvtf => "ucvtf",
            Fcvtzs => "fcvtzs",
            Fcvtzu => "fcvtzu",
        }
    }
}

impl ElemOp {
    /// The mnemonic (without the "2" of the upper-half forms).
    pub fn name(self) -> &'static str {
        use ElemOp::*;
        match self {
            Fmla => "fmla",
            Fmls => "fmls",
            Fmul => "fmul",
            Fmulx => "fmulx",
            Mla => "mla",
            Mls => "mls",
            Mul => "mul",
            Smlal => "smlal",
            Umlal => "umlal",
            Smlsl => "smlsl",
            Umlsl => "umlsl",
            Smull => "smull",
            Umull => "umull",
            Sqdmlal => "sqdmlal",
            Sqdmlsl => "sqdmlsl",
            Sqdmull => "sqdmull",
            Sqdmulh => "sqdmulh",
            Sqrdmulh => "sqrdmulh",
            Sqrdmlah => "sqrdmlah",
            Sqrdmlsh => "sqrdmlsh",
            Sdot => "sdot",
            Udot => "udot",
        }
    }
}

impl Sha3Op {
    /// The mnemonic.
    pub fn name(self) -> &'static str {
        use Sha3Op::*;
        match self {
            Sha1c => "sha1c",
            Sha1p => "sha1p",
            Sha1m => "sha1m",
            Sha1su0 => "sha1su0",
            Sha256h => "sha256h",
            Sha256h2 => "sha256h2",
            Sha256su1 => "sha256su1",
        }
    }
}
