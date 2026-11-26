//! The floating-point and Advanced SIMD encodings (the Arm ARM's C4.1.95 and its groups, and the SIMD structure loads
//! and stores of C4.1.88): each group matched by its fixed bits, its fields checked as the group's table allocates them.

use super::{b, bit, reg};
use crate::*;

fn vreg(w: u32, lo: u32) -> VReg {
    VReg(b(w, lo + 4, lo) as u8)
}

/// The `ftype` field of the scalar FP encodings: 00 S, 01 D, 11 H (10 is unallocated without BF16).
fn ftype(w: u32) -> Option<FpType> {
    match b(w, 23, 22) {
        0b00 => Some(FpType::S),
        0b01 => Some(FpType::D),
        0b11 => Some(FpType::H),
        _ => None,
    }
}

/// The arrangement of the `size` and `Q` fields, `None` for 1D where the form has no 1D (`no_1d`).
fn arr(w: u32, size: u32, no_1d: bool) -> Option<Arr> {
    let q = bit(w, 30);
    if no_1d && size == 3 && !q {
        return None;
    }
    Some(Arr::new(Size(size as u8), q))
}

/// The data-processing encodings of op0 x111 (bits 28:25).
pub(super) fn data(w: u32) -> Option<Insn> {
    if b(w, 30, 28) & 0b101 == 0b001 {
        return scalar_fp(w);
    }
    let m = |mask: u32, value: u32| w & mask == value;
    // (vector groups: bit 31 clear, bit 28 clear)
    if m(0xfffe_0c00, 0x4e28_0800) {
        return aes(w);
    }
    if m(0x9f20_0400, 0x0e20_0400) {
        return three_same(w, false);
    }
    if m(0x9f20_0c00, 0x0e20_0000) {
        return three_different(w, false);
    }
    if m(0x9f3e_0c00, 0x0e20_0800) {
        return two_misc(w, false);
    }
    if m(0x9f3e_0c00, 0x0e30_0800) {
        return across(w);
    }
    if m(0x9f60_c400, 0x0e40_0400) {
        return three_same_fp16(w, false);
    }
    if m(0x9f7e_0c00, 0x0e78_0800) {
        return two_misc_fp16(w, false);
    }
    if m(0x9f20_8400, 0x0e00_8400) {
        return three_same_extra(w, false);
    }
    if m(0x9fe0_8400, 0x0e00_0400) {
        return copy(w);
    }
    if m(0xbf20_8c00, 0x0e00_0000) {
        return table(w);
    }
    if m(0xbf20_8c00, 0x0e00_0800) {
        return permute(w);
    }
    if m(0xbf20_8400, 0x2e00_0000) {
        return extract(w);
    }
    if m(0x9ff8_0400, 0x0f00_0400) {
        return modified_immediate(w);
    }
    if m(0x9f80_0400, 0x0f00_0400) {
        return shift_immediate(w, false);
    }
    if m(0x9f00_0400, 0x0f00_0000) {
        return indexed(w, false);
    }
    // (scalar groups: bits 31:30 01, bit 28 set)
    if m(0xffe0_8c00, 0x5e00_0000) {
        return sha3(w);
    }
    if m(0xfffe_0c00, 0x5e28_0800) {
        return sha2(w);
    }
    if m(0xdf20_0400, 0x5e20_0400) {
        return three_same(w, true);
    }
    if m(0xdf20_0c00, 0x5e20_0000) {
        return three_different(w, true);
    }
    if m(0xdf3e_0c00, 0x5e20_0800) {
        return two_misc(w, true);
    }
    if m(0xdf3e_0c00, 0x5e30_0800) {
        return pairwise(w);
    }
    if m(0xdf60_c400, 0x5e40_0400) {
        return three_same_fp16(w, true);
    }
    if m(0xdf7e_0c00, 0x5e78_0800) {
        return two_misc_fp16(w, true);
    }
    if m(0xdf20_8400, 0x5e00_8400) {
        return three_same_extra(w, true);
    }
    if m(0xdfe0_8400, 0x5e00_0400) {
        return scalar_copy(w);
    }
    if m(0xdf80_0400, 0x5f00_0400) {
        return shift_immediate(w, true);
    }
    if m(0xdf00_0400, 0x5f00_0000) {
        return indexed(w, true);
    }
    None // (SHA512, SHA3, SM3, SM4 and the rest of op0 1xx0)
}

// ---- three registers

/// The FP operations of the three-same groups by U, a (size<1>) and the low 3 bits of the opcode.
fn fp_three_same(u: bool, a: bool, opc: u32) -> Option<Simd3Op> {
    use Simd3Op::*;
    let table: [[Option<Simd3Op>; 8]; 4] = [
        [Some(Fmaxnm), Some(Fmla), Some(Fadd), Some(Fmulx), Some(Fcmeq), None, Some(Fmax), Some(Frecps)],
        [Some(Fminnm), Some(Fmls), Some(Fsub), None, None, None, Some(Fmin), Some(Frsqrts)],
        [Some(Fmaxnmp), None, Some(Faddp), Some(Fmul), Some(Fcmge), Some(Facge), Some(Fmaxp), Some(Fdiv)],
        [Some(Fminnmp), None, Some(Fabd), None, Some(Fcmgt), Some(Facgt), Some(Fminp), None],
    ];
    table[usize::from(u) * 2 + usize::from(a)][opc as usize]
}

/// Whether an FP three-same operation has a scalar form.
fn fp_three_same_scalar(op: Simd3Op) -> bool {
    use Simd3Op::*;
    matches!(op, Fmulx | Fcmeq | Frecps | Frsqrts | Fcmge | Facge | Fabd | Fcmgt | Facgt)
}

fn three_same(w: u32, scalar: bool) -> Option<Insn> {
    use Simd3Op::*;
    let (u, size, opcode) = (bit(w, 29), b(w, 23, 22), b(w, 15, 11));
    let (rd, rn, rm) = (vreg(w, 0), vreg(w, 5), vreg(w, 16));
    if opcode >= 0b11000 {
        let op = fp_three_same(u, size >> 1 == 1, opcode & 7)?;
        let fsize = Size(2 + (size & 1) as u8);
        let shape = if scalar {
            if !fp_three_same_scalar(op) {
                return None;
            }
            Shape::Scalar(fsize)
        } else {
            Shape::Vector(arr(w, u32::from(fsize.0), true)?)
        };
        return Some(Insn::Simd3 { op, shape, rd, rn, rm });
    }
    let op = match (u, opcode) {
        (false, 0b00011) => [And, Bic, Orr, Orn][size as usize],
        (true, 0b00011) => [Eor, Bsl, Bit, Bif][size as usize],
        (_, 0b10111) if u => return None,
        _ => {
            let pair = [
                (Shadd, Uhadd),
                (Sqadd, Uqadd),
                (Srhadd, Urhadd),
                (And, And),
                (Shsub, Uhsub),
                (Sqsub, Uqsub),
                (Cmgt, Cmhi),
                (Cmge, Cmhs),
                (Sshl, Ushl),
                (Sqshl, Uqshl),
                (Srshl, Urshl),
                (Sqrshl, Uqrshl),
                (Smax, Umax),
                (Smin, Umin),
                (Sabd, Uabd),
                (Saba, Uaba),
                (Add, Sub),
                (Cmtst, Cmeq),
                (Mla, Mls),
                (Mul, Pmul),
                (Smaxp, Umaxp),
                (Sminp, Uminp),
                (Sqdmulh, Sqrdmulh),
                (Addp, Addp),
            ][opcode as usize];
            if u { pair.1 } else { pair.0 }
        }
    };
    let logical = opcode == 0b00011;
    let size_ok = match op {
        _ if logical => true,
        Mul | Mla | Mls => size != 3,
        Pmul => size == 0,
        Sqdmulh | Sqrdmulh => size == 1 || size == 2,
        Shadd | Uhadd | Srhadd | Urhadd | Shsub | Uhsub | Smax | Umax | Smin | Umin | Sabd | Uabd | Saba | Uaba
        | Smaxp | Umaxp | Sminp | Uminp => size != 3,
        _ => true,
    };
    if !size_ok {
        return None;
    }
    let shape = if scalar {
        let ok = match op {
            Sqadd | Uqadd | Sqsub | Uqsub | Sqshl | Uqshl | Sqrshl | Uqrshl => true,
            Cmgt | Cmhi | Cmge | Cmhs | Sshl | Ushl | Srshl | Urshl | Add | Sub | Cmtst | Cmeq => size == 3,
            Sqdmulh | Sqrdmulh => true,
            _ => false,
        };
        if !ok {
            return None;
        }
        Shape::Scalar(Size(size as u8))
    } else if logical {
        Shape::Vector(Arr::new(Size(0), bit(w, 30)))
    } else {
        Shape::Vector(arr(w, size, true)?)
    };
    Some(Insn::Simd3 { op, shape, rd, rn, rm })
}

fn three_same_fp16(w: u32, scalar: bool) -> Option<Insn> {
    let op = fp_three_same(bit(w, 29), bit(w, 23), b(w, 13, 11))?;
    let shape = if scalar {
        if !fp_three_same_scalar(op) {
            return None;
        }
        Shape::Scalar(Size(1))
    } else {
        Shape::Vector(Arr::new(Size(1), bit(w, 30)))
    };
    Some(Insn::Simd3 { op, shape, rd: vreg(w, 0), rn: vreg(w, 5), rm: vreg(w, 16) })
}

fn three_same_extra(w: u32, scalar: bool) -> Option<Insn> {
    let (u, size, opcode) = (bit(w, 29), b(w, 23, 22), b(w, 14, 11));
    let op = match (u, opcode) {
        (true, 0b0000) => Simd3Op::Sqrdmlah,
        (true, 0b0001) => Simd3Op::Sqrdmlsh,
        (false, 0b0010) if !scalar => Simd3Op::Sdot,
        (true, 0b0010) if !scalar => Simd3Op::Udot,
        _ => return None, // (USDOT: I8MM; FCMLA, FCADD: FCMA)
    };
    let shape = match op {
        Simd3Op::Sdot | Simd3Op::Udot if size == 2 => Shape::Vector(Arr::new(Size(2), bit(w, 30))),
        Simd3Op::Sqrdmlah | Simd3Op::Sqrdmlsh if size == 1 || size == 2 => {
            if scalar {
                Shape::Scalar(Size(size as u8))
            } else {
                Shape::Vector(Arr::new(Size(size as u8), bit(w, 30)))
            }
        }
        _ => return None,
    };
    Some(Insn::Simd3 { op, shape, rd: vreg(w, 0), rn: vreg(w, 5), rm: vreg(w, 16) })
}

fn three_different(w: u32, scalar: bool) -> Option<Insn> {
    use DiffOp::*;
    let (u, size, opcode) = (bit(w, 29), b(w, 23, 22), b(w, 15, 12));
    let pair = [
        (Some(Saddl), Some(Uaddl)),
        (Some(Saddw), Some(Uaddw)),
        (Some(Ssubl), Some(Usubl)),
        (Some(Ssubw), Some(Usubw)),
        (Some(Addhn), Some(Raddhn)),
        (Some(Sabal), Some(Uabal)),
        (Some(Subhn), Some(Rsubhn)),
        (Some(Sabdl), Some(Uabdl)),
        (Some(Smlal), Some(Umlal)),
        (Some(Sqdmlal), None),
        (Some(Smlsl), Some(Umlsl)),
        (Some(Sqdmlsl), None),
        (Some(Smull), Some(Umull)),
        (Some(Sqdmull), None),
        (Some(Pmull), None),
        (None, None),
    ][opcode as usize];
    let op = if u { pair.1 } else { pair.0 }?;
    let size_ok = match op {
        Sqdmlal | Sqdmlsl | Sqdmull => size == 1 || size == 2,
        Pmull => size == 0 || size == 3,
        _ => size != 3,
    };
    if !size_ok {
        return None;
    }
    let shape = if scalar {
        if !matches!(op, Sqdmlal | Sqdmlsl | Sqdmull) {
            return None;
        }
        Shape::Scalar(Size(size as u8))
    } else {
        Shape::Vector(Arr::new(Size(size as u8), bit(w, 30)))
    };
    Some(Insn::SimdDiff { op, shape, rd: vreg(w, 0), rn: vreg(w, 5), rm: vreg(w, 16) })
}

// ---- two registers

/// The FP operations of the two-register miscellaneous groups by U, a (size<1>) and the opcode.
fn fp_misc(u: bool, a: bool, opcode: u32) -> Option<Simd2Op> {
    use Simd2Op::*;
    let int = |round, unsigned| Some(FcvtInt { round, unsigned });
    match (opcode, a, u) {
        (0b11000, false, false) => Some(Frint(Round::Nearest)),
        (0b11000, false, true) => Some(Frint(Round::Away)),
        (0b11000, true, false) => Some(Frint(Round::Plus)),
        (0b11001, false, false) => Some(Frint(Round::Minus)),
        (0b11001, false, true) => Some(Frint(Round::Exact)),
        (0b11001, true, false) => Some(Frint(Round::Zero)),
        (0b11001, true, true) => Some(Frint(Round::Current)),
        (0b11010, false, _) => int(Round::Nearest, u),
        (0b11010, true, _) => int(Round::Plus, u),
        (0b11011, false, _) => int(Round::Minus, u),
        (0b11011, true, _) => int(Round::Zero, u),
        (0b11100, false, _) => int(Round::Away, u),
        (0b11101, false, false) => Some(Scvtf),
        (0b11101, false, true) => Some(Ucvtf),
        (0b11101, true, false) => Some(Frecpe),
        (0b11101, true, true) => Some(Frsqrte),
        (0b01100, true, false) => Some(Fcmp0(CmpZero::Gt)),
        (0b01100, true, true) => Some(Fcmp0(CmpZero::Ge)),
        (0b01101, true, false) => Some(Fcmp0(CmpZero::Eq)),
        (0b01101, true, true) => Some(Fcmp0(CmpZero::Le)),
        (0b01110, true, false) => Some(Fcmp0(CmpZero::Lt)),
        (0b01111, true, false) => Some(Fabs),
        (0b01111, true, true) => Some(Fneg),
        (0b11111, true, false) => Some(Frecpx),
        (0b11111, true, true) => Some(Fsqrt),
        _ => None,
    }
}

/// Whether an FP two-register operation has a scalar form.
fn fp_misc_scalar(op: Simd2Op) -> bool {
    use Simd2Op::*;
    matches!(op, FcvtInt { .. } | Scvtf | Ucvtf | Frecpe | Frsqrte | Frecpx | Fcmp0(_))
}

/// Whether an FP two-register operation has a vector form.
fn fp_misc_vector(op: Simd2Op) -> bool {
    op != Simd2Op::Frecpx
}

fn two_misc(w: u32, scalar: bool) -> Option<Insn> {
    use Simd2Op::*;
    let (u, size, opcode, q) = (bit(w, 29), b(w, 23, 22), b(w, 16, 12), bit(w, 30));
    let (rd, rn) = (vreg(w, 0), vreg(w, 5));
    let fsize = Size(2 + (size & 1) as u8);
    let vector = |s: u32, no_1d: bool| arr(w, s, no_1d).map(Shape::Vector);
    // (FCVTN, FCVTXN, FCVTL, URECPE, URSQRTE: the FP-range opcodes with integer or narrow elements)
    match (opcode, u, size >> 1) {
        (0b10110, false, 0) if !scalar => {
            let shape = Shape::Vector(Arr::new(Size(1 + (size & 1) as u8), q));
            return Some(Insn::Simd2 { op: Fcvtn, shape, rd, rn });
        }
        (0b10110, true, 0) if size & 1 == 1 => {
            let shape = if scalar { Shape::Scalar(Size(2)) } else { Shape::Vector(Arr::new(Size(2), q)) };
            return Some(Insn::Simd2 { op: Fcvtxn, shape, rd, rn });
        }
        (0b10111, false, 0) if !scalar => {
            let shape = Shape::Vector(Arr::new(Size(1 + (size & 1) as u8), q));
            return Some(Insn::Simd2 { op: Fcvtl, shape, rd, rn });
        }
        (0b11100, _, 1) if !scalar => {
            if size & 1 == 1 {
                return None;
            }
            let op = if u { Ursqrte } else { Urecpe };
            return Some(Insn::Simd2 { op, shape: Shape::Vector(Arr::new(Size(2), q)), rd, rn });
        }
        _ => {}
    }
    if opcode >= 0b11000 || (0b01100..=0b01111).contains(&opcode) {
        let op = fp_misc(u, size >> 1 == 1, opcode)?;
        let shape = if scalar {
            if !fp_misc_scalar(op) {
                return None;
            }
            Shape::Scalar(fsize)
        } else {
            if !fp_misc_vector(op) {
                return None;
            }
            vector(u32::from(fsize.0), true)?
        };
        return Some(Insn::Simd2 { op, shape, rd, rn });
    }
    let op = match (u, opcode) {
        (false, 0b00000) => Rev64,
        (false, 0b00001) => Rev16,
        (false, 0b00010) => Saddlp,
        (false, 0b00011) => Suqadd,
        (false, 0b00100) => Cls,
        (false, 0b00101) => Cnt,
        (false, 0b00110) => Sadalp,
        (false, 0b00111) => Sqabs,
        (false, 0b01000) => Cmp0(CmpZero::Gt),
        (false, 0b01001) => Cmp0(CmpZero::Eq),
        (false, 0b01010) => Cmp0(CmpZero::Lt),
        (false, 0b01011) => Abs,
        (false, 0b10010) => Xtn,
        (false, 0b10100) => Sqxtn,
        (true, 0b00000) => Rev32,
        (true, 0b00010) => Uaddlp,
        (true, 0b00011) => Usqadd,
        (true, 0b00100) => Clz,
        (true, 0b00101) if size == 0 => Not,
        (true, 0b00101) if size == 1 => Rbit,
        (true, 0b00110) => Uadalp,
        (true, 0b00111) => Sqneg,
        (true, 0b01000) => Cmp0(CmpZero::Ge),
        (true, 0b01001) => Cmp0(CmpZero::Le),
        (true, 0b01011) => Neg,
        (true, 0b10010) => Sqxtun,
        (true, 0b10011) => Shll,
        (true, 0b10100) => Uqxtn,
        _ => return None,
    };
    let shape = if scalar {
        match op {
            Suqadd | Usqadd | Sqabs | Sqneg => Shape::Scalar(Size(size as u8)),
            Cmp0(_) | Abs | Neg if size == 3 => Shape::Scalar(Size(3)),
            Sqxtn | Uqxtn | Sqxtun if size != 3 => Shape::Scalar(Size(size as u8)),
            _ => return None,
        }
    } else {
        match op {
            Rev64 | Cls | Clz | Saddlp | Uaddlp | Sadalp | Uadalp | Shll | Xtn | Sqxtn | Uqxtn | Sqxtun
                if size == 3 =>
            {
                return None;
            }
            Rev32 if size >= 2 => return None,
            Rev16 | Cnt if size != 0 => return None,
            Not | Rbit => Shape::Vector(Arr::new(Size(0), q)),
            Xtn | Sqxtn | Uqxtn | Sqxtun | Shll | Saddlp | Uaddlp | Sadalp | Uadalp => {
                Shape::Vector(Arr::new(Size(size as u8), q))
            }
            _ => vector(size, true)?,
        }
    };
    Some(Insn::Simd2 { op, shape, rd, rn })
}

fn two_misc_fp16(w: u32, scalar: bool) -> Option<Insn> {
    let op = fp_misc(bit(w, 29), bit(w, 23), b(w, 16, 12))?;
    let shape = if scalar {
        if !fp_misc_scalar(op) {
            return None;
        }
        Shape::Scalar(Size(1))
    } else {
        if !fp_misc_vector(op) {
            return None;
        }
        Shape::Vector(Arr::new(Size(1), bit(w, 30)))
    };
    Some(Insn::Simd2 { op, shape, rd: vreg(w, 0), rn: vreg(w, 5) })
}

fn across(w: u32) -> Option<Insn> {
    use AcrossOp::*;
    let (u, size, opcode, q) = (bit(w, 29), b(w, 23, 22), b(w, 16, 12), bit(w, 30));
    let (rd, rn) = (vreg(w, 0), vreg(w, 5));
    let fp = |max: AcrossOp, min: AcrossOp| {
        let op = if size >> 1 == 1 { min } else { max };
        let arr = if u {
            if size & 1 == 1 || !q {
                return None;
            }
            Arr::new(Size(2), true)
        } else {
            if size & 1 == 1 {
                return None;
            }
            Arr::new(Size(1), q)
        };
        Some(Insn::SimdAcross { op, arr, rd, rn })
    };
    let op = match (u, opcode) {
        (_, 0b01100) => return fp(Fmaxnmv, Fminnmv),
        (_, 0b01111) => return fp(Fmaxv, Fminv),
        (false, 0b00011) => Saddlv,
        (true, 0b00011) => Uaddlv,
        (false, 0b01010) => Smaxv,
        (true, 0b01010) => Umaxv,
        (false, 0b11010) => Sminv,
        (true, 0b11010) => Uminv,
        (false, 0b11011) => Addv,
        _ => return None,
    };
    if size == 3 || (size == 2 && !q) {
        return None;
    }
    Some(Insn::SimdAcross { op, arr: Arr::new(Size(size as u8), q), rd, rn })
}

fn pairwise(w: u32) -> Option<Insn> {
    use PairwiseOp::*;
    let (u, size, opcode) = (bit(w, 29), b(w, 23, 22), b(w, 16, 12));
    let a = size >> 1 == 1;
    let op = match (opcode, a) {
        (0b11011, _) if !u => {
            if size != 3 {
                return None;
            }
            return Some(Insn::SimdPairwise { op: Addp, size: Size(3), rd: vreg(w, 0), rn: vreg(w, 5) });
        }
        (0b01100, false) => Fmaxnmp,
        (0b01100, true) => Fminnmp,
        (0b01101, false) => Faddp,
        (0b01111, false) => Fmaxp,
        (0b01111, true) => Fminp,
        _ => return None,
    };
    let size = if u {
        Size(2 + (size & 1) as u8)
    } else {
        if size & 1 == 1 {
            return None;
        }
        Size(1)
    };
    Some(Insn::SimdPairwise { op, size, rd: vreg(w, 0), rn: vreg(w, 5) })
}

// ---- copies, permutes, tables

/// imm5's element size and index (`None` for the reserved x0000).
fn imm5(w: u32) -> Option<(Size, u8)> {
    let imm5 = b(w, 20, 16);
    let size = imm5.trailing_zeros();
    if size >= 4 {
        return None;
    }
    Some((Size(size as u8), (imm5 >> (size + 1)) as u8))
}

fn copy(w: u32) -> Option<Insn> {
    let (q, op, imm4) = (bit(w, 30), bit(w, 29), b(w, 14, 11));
    let (size, index) = imm5(w)?;
    let (rd, rn) = (vreg(w, 0), vreg(w, 5));
    if op {
        if !q {
            return None;
        }
        let src = (imm4 >> size.0) as u8;
        return Some(Insn::SimdInsElem { size, rd, dst: index, rn, src });
    }
    let x = Reg(b(w, 9, 5) as u8);
    let rdx = Reg(b(w, 4, 0) as u8);
    match imm4 {
        0b0000 => Some(Insn::SimdDupElem { shape: Shape::Vector(arr(w, u32::from(size.0), true)?), rd, rn, index }),
        0b0001 => Some(Insn::SimdDupGen { arr: arr(w, u32::from(size.0), true)?, rd, rn: x }),
        0b0011 if q => Some(Insn::SimdInsGen { size, rd, index, rn: x }),
        0b0101 if size.0 < 2 || (q && size.0 == 2) => {
            Some(Insn::SimdMovToGen { signed: true, x64: q, size, rd: rdx, rn, index })
        }
        0b0111 if (!q && size.0 < 3) || (q && size.0 == 3) => {
            Some(Insn::SimdMovToGen { signed: false, x64: q, size, rd: rdx, rn, index })
        }
        _ => None,
    }
}

fn scalar_copy(w: u32) -> Option<Insn> {
    if bit(w, 29) || b(w, 14, 11) != 0 {
        return None;
    }
    let (size, index) = imm5(w)?;
    Some(Insn::SimdDupElem { shape: Shape::Scalar(size), rd: vreg(w, 0), rn: vreg(w, 5), index })
}

fn permute(w: u32) -> Option<Insn> {
    let (op, second) = match b(w, 14, 12) {
        0b001 => (PermuteOp::Uzp, false),
        0b010 => (PermuteOp::Trn, false),
        0b011 => (PermuteOp::Zip, false),
        0b101 => (PermuteOp::Uzp, true),
        0b110 => (PermuteOp::Trn, true),
        0b111 => (PermuteOp::Zip, true),
        _ => return None,
    };
    let arr = arr(w, b(w, 23, 22), true)?;
    Some(Insn::SimdPermute { op, second, arr, rd: vreg(w, 0), rn: vreg(w, 5), rm: vreg(w, 16) })
}

fn extract(w: u32) -> Option<Insn> {
    let (q, imm4) = (bit(w, 30), b(w, 14, 11));
    if b(w, 23, 22) != 0 || (!q && imm4 >= 8) {
        return None;
    }
    Some(Insn::SimdExt { q, rd: vreg(w, 0), rn: vreg(w, 5), rm: vreg(w, 16), index: imm4 as u8 })
}

fn table(w: u32) -> Option<Insn> {
    if b(w, 23, 22) != 0 {
        return None;
    }
    Some(Insn::SimdTable {
        q: bit(w, 30),
        tbx: bit(w, 12),
        regs: b(w, 14, 13) as u8 + 1,
        rd: vreg(w, 0),
        rn: vreg(w, 5),
        rm: vreg(w, 16),
    })
}

fn modified_immediate(w: u32) -> Option<Insn> {
    use ModImmOp::*;
    let (q, op, cmode, o2) = (bit(w, 30), bit(w, 29), b(w, 15, 12), bit(w, 11));
    let imm8 = ((b(w, 18, 16) << 5) | b(w, 9, 5)) as u8;
    let mk = |op, size: u8, shift| Some(ModImm { op, arr: Arr::new(Size(size), q), imm8, shift });
    let imm = if o2 {
        if cmode != 0b1111 || op {
            return None;
        }
        mk(Fmov, 1, ModShift::Float)
    } else {
        let lsl = |n: u32| ModShift::Lsl(8 * n as u8);
        match (cmode, op) {
            (c, false) if c & 0b1001 == 0b0000 => mk(Movi, 2, lsl(c >> 1 & 3)),
            (c, true) if c & 0b1001 == 0b0000 => mk(Mvni, 2, lsl(c >> 1 & 3)),
            (c, false) if c & 0b1001 == 0b0001 => mk(Orr, 2, lsl(c >> 1 & 3)),
            (c, true) if c & 0b1001 == 0b0001 => mk(Bic, 2, lsl(c >> 1 & 3)),
            (c, false) if c & 0b1101 == 0b1000 => mk(Movi, 1, lsl(c >> 1 & 1)),
            (c, true) if c & 0b1101 == 0b1000 => mk(Mvni, 1, lsl(c >> 1 & 1)),
            (c, false) if c & 0b1101 == 0b1001 => mk(Orr, 1, lsl(c >> 1 & 1)),
            (c, true) if c & 0b1101 == 0b1001 => mk(Bic, 1, lsl(c >> 1 & 1)),
            (c, false) if c & 0b1110 == 0b1100 => mk(Movi, 2, ModShift::Msl(8 << (c & 1))),
            (c, true) if c & 0b1110 == 0b1100 => mk(Mvni, 2, ModShift::Msl(8 << (c & 1))),
            (0b1110, false) => mk(Movi, 0, ModShift::Lsl(0)),
            (0b1110, true) => mk(Movi, 3, ModShift::Bytes),
            (0b1111, false) => mk(Fmov, 2, ModShift::Float),
            (0b1111, true) if q => mk(Fmov, 3, ModShift::Float),
            _ => None,
        }
    }?;
    Some(Insn::SimdModImm { imm, rd: vreg(w, 0) })
}

// ---- shifts and elements

fn shift_immediate(w: u32, scalar: bool) -> Option<Insn> {
    use ShiftOp::*;
    let (q, u, immh, opcode) = (bit(w, 30), bit(w, 29), b(w, 22, 19), b(w, 15, 11));
    let immhb = b(w, 22, 16);
    if immh == 0 {
        return None; // (the modified immediates, or unallocated)
    }
    let size = 31 - immh.leading_zeros();
    let esize = 8u32 << size;
    let op = match (u, opcode) {
        (false, 0b00000) => Sshr,
        (false, 0b00010) => Ssra,
        (false, 0b00100) => Srshr,
        (false, 0b00110) => Srsra,
        (false, 0b01010) => Shl,
        (false, 0b01110) => Sqshl,
        (false, 0b10000) => Shrn,
        (false, 0b10001) => Rshrn,
        (false, 0b10010) => Sqshrn,
        (false, 0b10011) => Sqrshrn,
        (false, 0b10100) => Sshll,
        (false, 0b11100) => Scvtf,
        (false, 0b11111) => Fcvtzs,
        (true, 0b00000) => Ushr,
        (true, 0b00010) => Usra,
        (true, 0b00100) => Urshr,
        (true, 0b00110) => Ursra,
        (true, 0b01000) => Sri,
        (true, 0b01010) => Sli,
        (true, 0b01100) => Sqshlu,
        (true, 0b01110) => Uqshl,
        (true, 0b10000) => Sqshrun,
        (true, 0b10001) => Sqrshrun,
        (true, 0b10010) => Uqshrn,
        (true, 0b10011) => Uqrshrn,
        (true, 0b10100) => Ushll,
        (true, 0b11100) => Ucvtf,
        (true, 0b11111) => Fcvtzu,
        _ => return None,
    };
    let fixed = matches!(op, Scvtf | Ucvtf | Fcvtzs | Fcvtzu);
    if (op.narrows() || op.widens()) && size == 3 {
        return None;
    }
    if fixed && size == 0 {
        return None;
    }
    let shift = if op.right() { 2 * esize - immhb } else { immhb - esize } as u8;
    let shape = if scalar {
        let ok = match op {
            Sshr | Ushr | Ssra | Usra | Srshr | Urshr | Srsra | Ursra | Sri | Shl | Sli => size == 3,
            Shrn | Rshrn | Sshll | Ushll => false,
            _ => true,
        };
        if !ok {
            return None;
        }
        Shape::Scalar(Size(size as u8))
    } else if op.narrows() || op.widens() {
        Shape::Vector(Arr::new(Size(size as u8), q))
    } else {
        Shape::Vector(arr(w, size, true)?)
    };
    Some(Insn::SimdShift { op, shape, rd: vreg(w, 0), rn: vreg(w, 5), shift })
}

fn indexed(w: u32, scalar: bool) -> Option<Insn> {
    use ElemOp::*;
    let (q, u, size, opcode) = (bit(w, 30), bit(w, 29), b(w, 23, 22), b(w, 15, 12));
    let (h, l, m) = (b(w, 11, 11), b(w, 21, 21), b(w, 20, 20));
    let rm4 = b(w, 19, 16);
    let op = match (u, opcode) {
        (false, 0b0001) => Fmla,
        (false, 0b0101) => Fmls,
        (false, 0b1001) => Fmul,
        (true, 0b1001) => Fmulx,
        (true, 0b0000) => Mla,
        (true, 0b0100) => Mls,
        (false, 0b1000) => Mul,
        (false, 0b0010) => Smlal,
        (true, 0b0010) => Umlal,
        (false, 0b0110) => Smlsl,
        (true, 0b0110) => Umlsl,
        (false, 0b1010) => Smull,
        (true, 0b1010) => Umull,
        (false, 0b0011) => Sqdmlal,
        (false, 0b0111) => Sqdmlsl,
        (false, 0b1011) => Sqdmull,
        (false, 0b1100) => Sqdmulh,
        (false, 0b1101) => Sqrdmulh,
        (true, 0b1101) => Sqrdmlah,
        (true, 0b1111) => Sqrdmlsh,
        (false, 0b1110) => Sdot,
        (true, 0b1110) => Udot,
        _ => return None, // (FMLAL, FMLSL: FHM; FCMLA: FCMA; SUDOT, USDOT: I8MM)
    };
    let fp = matches!(op, Fmla | Fmls | Fmul | Fmulx);
    // (index and Rm: H:L:M and Rm<3:0> for halfwords, H:L and M:Rm for words, H and M:Rm for doublewords)
    let (esize, index, rm) = match (size, fp) {
        (0, true) | (1, false) => (1, (h << 2) | (l << 1) | m, rm4),
        (2, _) => (2, (h << 1) | l, (m << 4) | rm4),
        (3, true) if l == 0 => (3, h, (m << 4) | rm4),
        _ => return None,
    };
    let dot = matches!(op, Sdot | Udot);
    if dot && size != 2 {
        return None;
    }
    let shape = if scalar {
        if matches!(op, Mla | Mls | Mul | Smlal | Umlal | Smlsl | Umlsl | Smull | Umull | Sdot | Udot) {
            return None;
        }
        Shape::Scalar(Size(esize))
    } else if op.widens() || dot {
        Shape::Vector(Arr::new(Size(esize), q))
    } else {
        Shape::Vector(arr(w, u32::from(esize), true)?)
    };
    Some(Insn::SimdElem { op, shape, rd: vreg(w, 0), rn: vreg(w, 5), rm: VReg(rm as u8), index: index as u8 })
}

// ---- cryptography

fn aes(w: u32) -> Option<Insn> {
    let op = match b(w, 16, 12) {
        0b00100 => AesOp::Aese,
        0b00101 => AesOp::Aesd,
        0b00110 => AesOp::Aesmc,
        0b00111 => AesOp::Aesimc,
        _ => return None,
    };
    Some(Insn::Aes { op, rd: vreg(w, 0), rn: vreg(w, 5) })
}

fn sha3(w: u32) -> Option<Insn> {
    use Sha3Op::*;
    let op = [Sha1c, Sha1p, Sha1m, Sha1su0, Sha256h, Sha256h2, Sha256su1].get(b(w, 14, 12) as usize).copied()?;
    Some(Insn::Sha3 { op, rd: vreg(w, 0), rn: vreg(w, 5), rm: vreg(w, 16) })
}

fn sha2(w: u32) -> Option<Insn> {
    let op = match b(w, 16, 12) {
        0b00000 => Sha2Op::Sha1h,
        0b00001 => Sha2Op::Sha1su1,
        0b00010 => Sha2Op::Sha256su0,
        _ => return None,
    };
    Some(Insn::Sha2 { op, rd: vreg(w, 0), rn: vreg(w, 5) })
}

// ---- scalar floating point

fn scalar_fp(w: u32) -> Option<Insn> {
    let (m, s) = (bit(w, 31), bit(w, 29));
    if s {
        return None;
    }
    if bit(w, 24) {
        return (!m).then(|| fp3(w)).flatten();
    }
    if !bit(w, 21) {
        return fixed_conversion(w);
    }
    if b(w, 15, 10) == 0 {
        return int_conversion(w);
    }
    if m {
        return None;
    }
    let ty = ftype(w)?;
    let (rd, rn, rm) = (vreg(w, 0), vreg(w, 5), vreg(w, 16));
    if b(w, 14, 10) == 0b10000 {
        return fp1(w, ty);
    }
    if b(w, 13, 10) == 0b1000 {
        if b(w, 15, 14) != 0 || b(w, 2, 0) != 0 {
            return None;
        }
        let zero = bit(w, 3);
        return Some(Insn::FpCmp { ty, signaling: bit(w, 4), rn, rm: if zero { None } else { Some(rm) } });
    }
    if b(w, 12, 10) == 0b100 {
        if b(w, 9, 5) != 0 {
            return None;
        }
        return Some(Insn::FpImm { ty, rd, imm8: b(w, 20, 13) as u8 });
    }
    let cond = Cond(b(w, 15, 12) as u8);
    match b(w, 11, 10) {
        0b01 => Some(Insn::FpCondCmp { ty, signaling: bit(w, 4), rn, rm, nzcv: b(w, 3, 0) as u8, cond }),
        0b10 => {
            let op = match b(w, 15, 12) {
                0b0000 => Fp2Op::Mul,
                0b0001 => Fp2Op::Div,
                0b0010 => Fp2Op::Add,
                0b0011 => Fp2Op::Sub,
                0b0100 => Fp2Op::Max,
                0b0101 => Fp2Op::Min,
                0b0110 => Fp2Op::MaxNm,
                0b0111 => Fp2Op::MinNm,
                0b1000 => Fp2Op::NMul,
                _ => return None,
            };
            Some(Insn::Fp2 { op, ty, rd, rn, rm })
        }
        0b11 => Some(Insn::FpCsel { ty, rd, rn, rm, cond }),
        _ => None,
    }
}

fn fp1(w: u32, ty: FpType) -> Option<Insn> {
    let (rd, rn) = (vreg(w, 0), vreg(w, 5));
    let op = match b(w, 20, 15) {
        0b000000 => Fp1Op::Mov,
        0b000001 => Fp1Op::Abs,
        0b000010 => Fp1Op::Neg,
        0b000011 => Fp1Op::Sqrt,
        o @ 0b000100..=0b000111 => {
            let to = match o & 3 {
                0b00 => FpType::S,
                0b01 => FpType::D,
                0b11 => FpType::H,
                _ => return None,
            };
            return (to != ty).then_some(Insn::FpCvt { from: ty, to, rd, rn });
        }
        0b001000 => Fp1Op::Rint(Round::Nearest),
        0b001001 => Fp1Op::Rint(Round::Plus),
        0b001010 => Fp1Op::Rint(Round::Minus),
        0b001011 => Fp1Op::Rint(Round::Zero),
        0b001100 => Fp1Op::Rint(Round::Away),
        0b001110 => Fp1Op::Rint(Round::Exact),
        0b001111 => Fp1Op::Rint(Round::Current),
        _ => return None, // (FRINT32Z ... FRINT64X: v8.5; BFCVT: BF16)
    };
    Some(Insn::Fp1 { op, ty, rd, rn })
}

fn fp3(w: u32) -> Option<Insn> {
    let ty = ftype(w)?;
    let op = match (bit(w, 21), bit(w, 15)) {
        (false, false) => Fp3Op::Madd,
        (false, true) => Fp3Op::Msub,
        (true, false) => Fp3Op::Nmadd,
        (true, true) => Fp3Op::Nmsub,
    };
    Some(Insn::Fp3 { op, ty, rd: vreg(w, 0), rn: vreg(w, 5), rm: vreg(w, 16), ra: vreg(w, 10) })
}

fn fixed_conversion(w: u32) -> Option<Insn> {
    let sf = bit(w, 31);
    let ty = ftype(w)?;
    let scale = b(w, 15, 10);
    if !sf && scale < 32 {
        return None;
    }
    let fbits = (64 - scale) as u8;
    match (b(w, 20, 19), b(w, 18, 16)) {
        (0b00, o @ (0b010 | 0b011)) => {
            Some(Insn::IntToFp { ty, sf, unsigned: o & 1 == 1, fbits, rd: vreg(w, 0), rn: reg(w, 5) })
        }
        (0b11, o @ (0b000 | 0b001)) => Some(Insn::FpToInt {
            ty,
            sf,
            round: Round::Zero,
            unsigned: o & 1 == 1,
            fbits,
            rd: reg(w, 0),
            rn: vreg(w, 5),
        }),
        _ => None,
    }
}

fn int_conversion(w: u32) -> Option<Insn> {
    let sf = bit(w, 31);
    let (rmode, opcode) = (b(w, 20, 19), b(w, 18, 16));
    if opcode >= 0b110 {
        // FMOV (general)
        let part = match (sf, b(w, 23, 22), rmode) {
            (false, 0b11, 0b00) => FmovPart::WH,
            (true, 0b11, 0b00) => FmovPart::XH,
            (false, 0b00, 0b00) => FmovPart::WS,
            (true, 0b01, 0b00) => FmovPart::XD,
            (true, 0b10, 0b01) => FmovPart::XTop,
            _ => return None,
        };
        let to_general = opcode == 0b110;
        let (gpr, fpr) = if to_general { (reg(w, 0), vreg(w, 5)) } else { (reg(w, 5), vreg(w, 0)) };
        return Some(Insn::FmovGen { part, to_general, gpr, fpr });
    }
    let ty = ftype(w)?;
    let unsigned = opcode & 1 == 1;
    let round = match (rmode, opcode >> 1) {
        (0b00, 0b00) => Round::Nearest,
        (0b01, 0b00) => Round::Plus,
        (0b10, 0b00) => Round::Minus,
        (0b11, 0b00) => Round::Zero,
        (0b00, 0b10) => Round::Away,
        (0b00, 0b01) => {
            return Some(Insn::IntToFp { ty, sf, unsigned, fbits: 0, rd: vreg(w, 0), rn: reg(w, 5) });
        }
        _ => return None, // (FJCVTZS: JSCVT)
    };
    Some(Insn::FpToInt { ty, sf, round, unsigned, fbits: 0, rd: reg(w, 0), rn: vreg(w, 5) })
}

// ---- structure loads and stores

/// The Advanced SIMD structure loads and stores (bits 31, 29:28 clear, 26 set).
pub(super) fn structure(w: u32) -> Option<Insn> {
    let (q, load, post) = (bit(w, 30), bit(w, 22), bit(w, 23));
    let (rt, rn, rm) = (vreg(w, 0), reg(w, 5), reg(w, 16));
    let what = if !bit(w, 24) {
        if bit(w, 21) || (!post && b(w, 20, 16) != 0) {
            return None;
        }
        let size = b(w, 11, 10);
        let (regs, selem) = match b(w, 15, 12) {
            0b0000 => (4, 4),
            0b0010 => (4, 1),
            0b0100 => (3, 3),
            0b0110 => (3, 1),
            0b0111 => (1, 1),
            0b1000 => (2, 2),
            0b1010 => (2, 1),
            _ => return None,
        };
        if selem > 1 && size == 3 && !q {
            return None;
        }
        Structure::Multiple { arr: Arr::new(Size(size as u8), q), regs, selem }
    } else {
        if !post && b(w, 20, 16) != 0 {
            return None;
        }
        let (opcode, s, size) = (b(w, 15, 13), b(w, 12, 12), b(w, 11, 10));
        let selem = (((opcode & 1) << 1) | b(w, 21, 21)) as u8 + 1;
        let qb = u32::from(q);
        match opcode >> 1 {
            0b11 => {
                if !load || s != 0 {
                    return None;
                }
                Structure::Replicate { arr: Arr::new(Size(size as u8), q), selem }
            }
            0b00 => Structure::Lane { size: Size(0), index: ((qb << 3) | (s << 2) | size) as u8, selem },
            0b01 => {
                if size & 1 != 0 {
                    return None;
                }
                Structure::Lane { size: Size(1), index: ((qb << 2) | (s << 1) | (size >> 1)) as u8, selem }
            }
            _ => match (size, s) {
                (0b00, _) => Structure::Lane { size: Size(2), index: ((qb << 1) | s) as u8, selem },
                (0b01, 0) => Structure::Lane { size: Size(3), index: qb as u8, selem },
                _ => return None,
            },
        }
    };
    let post = post.then(|| if rm.is_31() { PostIndex::Imm(what.bytes()) } else { PostIndex::Reg(rm) });
    Some(Insn::SimdLdSt { load, what, rt, rn, post })
}
