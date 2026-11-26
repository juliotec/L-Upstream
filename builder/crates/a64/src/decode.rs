//! Instruction word to [`Insn`], following the Arm ARM's top-level encoding table (C4.1) and each group's.

use crate::*;

mod simd;

#[inline]
fn b(w: u32, hi: u32, lo: u32) -> u32 {
    (w >> lo) & ((1u32 << (hi - lo + 1)) - 1)
}

#[inline]
fn bit(w: u32, n: u32) -> bool {
    (w >> n) & 1 != 0
}

/// `v`'s low `bits` bits as a signed number.
#[inline]
fn sext(v: u32, bits: u32) -> i64 {
    let shift = 64 - bits;
    (((v as u64) << shift) as i64) >> shift
}

#[inline]
fn reg(w: u32, lo: u32) -> Reg {
    Reg(b(w, lo + 4, lo) as u8)
}

/// The instruction `w` is, or `None` when it is unallocated, reserved, or needs a feature the A78C lacks.
pub fn decode(w: u32) -> Option<Insn> {
    match b(w, 28, 25) {
        0b0000 => reserved(w),
        0b1000 | 0b1001 => dp_imm(w),
        0b1010 | 0b1011 => branch_sys(w),
        0b0100 | 0b0110 | 0b1100 | 0b1110 => ldst(w),
        0b0101 | 0b1101 => dp_reg(w),
        0b0111 | 0b1111 => simd::data(w),
        _ => None, // (0001, 0011 unallocated; 0010 SVE)
    }
}

fn reserved(w: u32) -> Option<Insn> {
    // UDF: op0 (31) 0, 30:29 00, op1 (24:16) 0
    if b(w, 31, 29) == 0 && b(w, 24, 16) == 0 {
        return Some(Insn::Udf { imm: b(w, 15, 0) as u16 });
    }
    None
}

// ---- data processing, immediate

fn dp_imm(w: u32) -> Option<Insn> {
    let sf = bit(w, 31);
    let rd = reg(w, 0);
    let rn = reg(w, 5);
    match b(w, 25, 23) {
        0b000 | 0b001 => {
            let page = bit(w, 31);
            let imm = sext((b(w, 23, 5) << 2) | b(w, 30, 29), 21);
            Some(Insn::Adr { page, rd, imm: if page { imm << 12 } else { imm } })
        }
        0b010 => Some(Insn::AddSubImm {
            sf,
            sub: bit(w, 30),
            setflags: bit(w, 29),
            rd,
            rn,
            imm12: b(w, 21, 10) as u16,
            shift12: bit(w, 22),
        }),
        0b011 => None, // (ADDG, SUBG: MTE)
        0b100 => {
            let n = bit(w, 22);
            if !sf && n {
                return None;
            }
            let imm = decode_bit_masks(n, b(w, 15, 10), b(w, 21, 16), if sf { 64 } else { 32 })?;
            let op = [LogicOp::And, LogicOp::Orr, LogicOp::Eor, LogicOp::Ands][b(w, 30, 29) as usize];
            Some(Insn::LogicalImm { sf, op, rd, rn, imm })
        }
        0b101 => {
            let op = match b(w, 30, 29) {
                0b00 => MoveOp::Movn,
                0b10 => MoveOp::Movz,
                0b11 => MoveOp::Movk,
                _ => return None,
            };
            let hw = b(w, 22, 21) as u8;
            if !sf && hw >= 2 {
                return None;
            }
            Some(Insn::MoveWide { sf, op, rd, imm16: b(w, 20, 5) as u16, hw })
        }
        0b110 => {
            let op = match b(w, 30, 29) {
                0b00 => BitfieldOp::Sbfm,
                0b01 => BitfieldOp::Bfm,
                0b10 => BitfieldOp::Ubfm,
                _ => return None,
            };
            let (immr, imms) = (b(w, 21, 16) as u8, b(w, 15, 10) as u8);
            if bit(w, 22) != sf || (!sf && (immr | imms) & 0x20 != 0) {
                return None;
            }
            Some(Insn::Bitfield { sf, op, rd, rn, immr, imms })
        }
        _ => {
            let lsb = b(w, 15, 10) as u8;
            if b(w, 30, 29) != 0 || bit(w, 22) != sf || bit(w, 21) || (!sf && lsb >= 32) {
                return None;
            }
            Some(Insn::Extr { sf, rd, rn, rm: reg(w, 16), lsb })
        }
    }
}

// ---- branches, exception generation, system

fn branch_sys(w: u32) -> Option<Insn> {
    let op1 = b(w, 25, 12);
    match b(w, 31, 29) {
        0b010 => {
            if bit(w, 25) || bit(w, 24) || bit(w, 4) {
                return None; // (BC.cond: v8.8)
            }
            Some(Insn::BCond { cond: Cond(b(w, 3, 0) as u8), offset: sext(b(w, 23, 5), 19) * 4 })
        }
        0b000 | 0b100 => Some(Insn::B { link: bit(w, 31), offset: sext(b(w, 25, 0), 26) * 4 }),
        0b001 | 0b101 => {
            let rt = reg(w, 0);
            if !bit(w, 25) {
                Some(Insn::Cbz { sf: bit(w, 31), nonzero: bit(w, 24), rt, offset: sext(b(w, 23, 5), 19) * 4 })
            } else {
                let bitno = ((bit(w, 31) as u8) << 5) | b(w, 23, 19) as u8;
                Some(Insn::Tbz { nonzero: bit(w, 24), rt, bit: bitno, offset: sext(b(w, 18, 5), 14) * 4 })
            }
        }
        0b110 => {
            if bit(w, 25) {
                branch_reg(w)
            } else if b(w, 25, 24) == 0 {
                exception(w)
            } else if op1 == 0b01000000110010 {
                if b(w, 4, 0) != 0b11111 {
                    return None;
                }
                Some(Insn::Hint { imm: b(w, 11, 5) as u8 })
            } else if op1 == 0b01000000110011 {
                barrier(w)
            } else if op1 & 0b11111110001111 == 0b01000000000100 {
                pstate(w)
            } else if op1 & 0b11110110000000 == 0b01000010000000 {
                Some(Insn::Sys {
                    l: bit(w, 21),
                    op1: b(w, 18, 16) as u8,
                    crn: b(w, 15, 12) as u8,
                    crm: b(w, 11, 8) as u8,
                    op2: b(w, 7, 5) as u8,
                    rt: reg(w, 0),
                })
            } else if op1 & 0b11110100000000 == 0b01000100000000 {
                let r = SysReg(b(w, 20, 5) as u16);
                let rt = reg(w, 0);
                Some(if bit(w, 21) { Insn::Mrs { reg: r, rt } } else { Insn::Msr { reg: r, rt } })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn exception(w: u32) -> Option<Insn> {
    let imm = b(w, 20, 5) as u16;
    match (b(w, 23, 21), b(w, 4, 2), b(w, 1, 0)) {
        (0b000, 0, 0b01) => Some(Insn::Svc { imm }),
        (0b000, 0, 0b10) => Some(Insn::Hvc { imm }),
        (0b000, 0, 0b11) => Some(Insn::Smc { imm }),
        (0b001, 0, 0b00) => Some(Insn::Brk { imm }),
        (0b010, 0, 0b00) => Some(Insn::Hlt { imm }),
        (0b101, 0, ll @ 1..=3) => Some(Insn::Dcps { level: ll as u8, imm }),
        _ => None,
    }
}

fn barrier(w: u32) -> Option<Insn> {
    if b(w, 4, 0) != 0b11111 {
        return None;
    }
    let crm = b(w, 11, 8) as u8;
    match b(w, 7, 5) {
        0b010 => Some(Insn::Clrex { crm }),
        0b100 => Some(Insn::Barrier { op: Barrier::Dsb, crm }),
        0b101 => Some(Insn::Barrier { op: Barrier::Dmb, crm }),
        0b110 => Some(Insn::Barrier { op: Barrier::Isb, crm }),
        _ => None, // (DSB nXS: v8.7; TCOMMIT: TME; SB)
    }
}

fn pstate(w: u32) -> Option<Insn> {
    if b(w, 4, 0) != 0b11111 {
        return None;
    }
    let (op1, op2, crm) = (b(w, 18, 16) as u8, b(w, 7, 5) as u8, b(w, 11, 8) as u8);
    let known = match (op1, op2) {
        (0b000, 0b000) => true,                                   // CFINV (CRm should be zero)
        (0b000, 0b011) | (0b000, 0b100) | (0b000, 0b101) => true, // UAO, PAN, SPSel
        (0b011, 0b001) => true,                                   // SSBS (DIT: v8.4)
        (0b011, 0b110) | (0b011, 0b111) => true,                  // DAIFSet, DAIFClr
        _ => false,
    };
    known.then_some(Insn::Pstate { op1, op2, crm })
}

fn branch_reg(w: u32) -> Option<Insn> {
    if b(w, 20, 16) != 0b11111 {
        return None;
    }
    let (opc, op3, rn, op4) = (b(w, 24, 21), b(w, 15, 10), reg(w, 5), b(w, 4, 0));
    let pac = op3 >> 1 == 0b00001;
    let key_b = bit(w, 10);
    let plain = op3 == 0 && op4 == 0;
    let op = |op, rn, auth| Some(Insn::BranchReg { op, rn, auth });
    match opc {
        0b0000 | 0b0001 => {
            let o = if opc == 0 { BranchRegOp::Br } else { BranchRegOp::Blr };
            if plain {
                op(o, rn, BranchAuth::None)
            } else if pac && op4 == 0b11111 {
                op(o, rn, BranchAuth::Zero { b: key_b })
            } else {
                None
            }
        }
        0b1000 | 0b1001 => {
            let o = if opc == 0b1000 { BranchRegOp::Br } else { BranchRegOp::Blr };
            if pac { op(o, rn, BranchAuth::Reg { b: key_b, rm: Reg(op4 as u8) }) } else { None }
        }
        0b0010 => {
            if plain {
                op(BranchRegOp::Ret, rn, BranchAuth::None)
            } else if pac && rn.is_31() && op4 == 0b11111 {
                op(BranchRegOp::Ret, Reg(30), BranchAuth::Sp { b: key_b })
            } else {
                None
            }
        }
        0b0100 => {
            if plain && rn.is_31() {
                op(BranchRegOp::Eret, Reg(30), BranchAuth::None)
            } else if pac && rn.is_31() && op4 == 0b11111 {
                op(BranchRegOp::Eret, Reg(30), BranchAuth::Sp { b: key_b })
            } else {
                None
            }
        }
        0b0101 => (plain && rn.is_31()).then_some(Insn::BranchReg {
            op: BranchRegOp::Drps,
            rn: Reg(30),
            auth: BranchAuth::None,
        }),
        _ => None,
    }
}

// ---- loads and stores

fn ldst(w: u32) -> Option<Insn> {
    let v = bit(w, 26);
    let op2 = b(w, 24, 23);
    let op3 = b(w, 21, 16);
    let op4 = b(w, 11, 10);
    match b(w, 29, 28) {
        0b00 => {
            if v {
                // Advanced SIMD structure loads and stores (only with bit 31 clear), else unallocated
                return if bit(w, 31) { None } else { simd::structure(w) };
            }
            if op2 >> 1 == 0 { exclusive(w) } else { None }
        }
        0b01 => {
            if op2 >> 1 == 0 { literal(w) } else { None } // (LDAPUR, STLUR: RCpc-immo)
        }
        0b10 => pair(w),
        _ => {
            if op2 >> 1 == 1 {
                single(w, Mode::UnsignedImm)
            } else if op3 >> 5 == 0 {
                single(w, [Mode::Unscaled, Mode::Post, Mode::Unpriv, Mode::Pre][op4 as usize])
            } else {
                match op4 {
                    0b00 => atomic(w),
                    0b10 => single(w, Mode::RegOffset),
                    _ => ldr_pac(w),
                }
            }
        }
    }
}

fn exclusive(w: u32) -> Option<Insn> {
    // Rs of a load and Rt2 of a single-register form are should-be-one fields: decoded whatever they hold, as LLVM
    // and the hardware do.
    let size = b(w, 31, 30);
    let (o2, l, o1, o0) = (bit(w, 23), bit(w, 22), bit(w, 21), bit(w, 15));
    let (rs, rt2, rn, rt) = (reg(w, 16), reg(w, 10), reg(w, 5), reg(w, 0));
    match (o2, o1) {
        (false, false) => Some(Insn::Exclusive { load: l, ordered: o0, size: Size(size as u8), rs, rt, rt2: None, rn }),
        (false, true) => {
            if size >= 2 {
                Some(Insn::Exclusive { load: l, ordered: o0, size: Size(size as u8), rs, rt, rt2: Some(rt2), rn })
            } else {
                // CASP: even register pairs; Rt2 (should be one) is checked for CAS and CASP, as LLVM does
                if !rt2.is_31() || rs.0 & 1 != 0 || rt.0 & 1 != 0 {
                    return None;
                }
                let size = Size(2 + size as u8);
                Some(Insn::Cas { pair: true, size, acquire: l, release: o0, rs, rt, rn })
            }
        }
        (true, false) => Some(Insn::Ordered { load: l, limited: !o0, size: Size(size as u8), rt, rn }),
        (true, true) if rt2.is_31() => {
            Some(Insn::Cas { pair: false, size: Size(size as u8), acquire: l, release: o0, rs, rt, rn })
        }
        (true, true) => None,
    }
}

fn literal(w: u32) -> Option<Insn> {
    let (kind, size) = match (bit(w, 26), b(w, 31, 30)) {
        (false, 0b00) => (LsKind::Load, 2),
        (false, 0b01) => (LsKind::Load, 3),
        (false, 0b10) => (LsKind::LoadSigned { x64: true }, 2),
        (false, _) => (LsKind::Prefetch, 3),
        (true, 0b00) => (LsKind::LoadFp, 2),
        (true, 0b01) => (LsKind::LoadFp, 3),
        (true, 0b10) => (LsKind::LoadFp, 4),
        (true, _) => return None,
    };
    let addr = Addr::Literal { imm: sext(b(w, 23, 5), 19) * 4 };
    Some(Insn::LoadStore { kind, flavour: LsFlavour::Normal, size: Size(size), rt: reg(w, 0), addr })
}

fn pair(w: u32) -> Option<Insn> {
    let (opc, v, l) = (b(w, 31, 30), bit(w, 26), bit(w, 22));
    let nontemporal = b(w, 24, 23) == 0;
    let (kind, size) = match (v, opc) {
        (false, 0b00) => (if l { PairKind::Load } else { PairKind::Store }, 2),
        (false, 0b01) if l && !nontemporal => (PairKind::LoadSignedWord, 2),
        (false, 0b10) => (if l { PairKind::Load } else { PairKind::Store }, 3),
        (true, 0b00..=0b10) => (if l { PairKind::LoadFp } else { PairKind::StoreFp }, 2 + opc as u8),
        _ => return None, // (STGP: MTE)
    };
    let rn = reg(w, 5);
    let imm = sext(b(w, 21, 15), 7) << size;
    let addr = match b(w, 24, 23) {
        0b00 | 0b10 => Addr::Offset { rn, imm },
        0b01 => Addr::PostIndex { rn, imm },
        _ => Addr::PreIndex { rn, imm },
    };
    Some(Insn::Pair { kind, nontemporal, size: Size(size), rt: reg(w, 0), rt2: reg(w, 10), addr })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Unscaled,
    Post,
    Unpriv,
    Pre,
    RegOffset,
    UnsignedImm,
}

fn single(w: u32, mode: Mode) -> Option<Insn> {
    let (size, v, opc) = (b(w, 31, 30), bit(w, 26), b(w, 23, 22));
    let (kind, size) = if !v {
        match opc {
            0b00 => (LsKind::Store, size),
            0b01 => (LsKind::Load, size),
            0b10 if size == 3 => {
                if !matches!(mode, Mode::UnsignedImm | Mode::RegOffset | Mode::Unscaled) {
                    return None;
                }
                (LsKind::Prefetch, 3)
            }
            0b10 => (LsKind::LoadSigned { x64: true }, size),
            _ if size <= 1 => (LsKind::LoadSigned { x64: false }, size),
            _ => return None,
        }
    } else {
        if mode == Mode::Unpriv {
            return None;
        }
        let store = opc & 1 == 0;
        let size = if opc & 2 != 0 {
            if size != 0 {
                return None;
            }
            4
        } else {
            size
        };
        (if store { LsKind::StoreFp } else { LsKind::LoadFp }, size)
    };
    let rn = reg(w, 5);
    let imm9 = sext(b(w, 20, 12), 9);
    let (flavour, addr) = match mode {
        Mode::Unscaled => (LsFlavour::Unscaled, Addr::Offset { rn, imm: imm9 }),
        Mode::Unpriv => (LsFlavour::Unprivileged, Addr::Offset { rn, imm: imm9 }),
        Mode::Post => (LsFlavour::Normal, Addr::PostIndex { rn, imm: imm9 }),
        Mode::Pre => (LsFlavour::Normal, Addr::PreIndex { rn, imm: imm9 }),
        Mode::UnsignedImm => (LsFlavour::Normal, Addr::Offset { rn, imm: (b(w, 21, 10) as i64) << size }),
        Mode::RegOffset => {
            let option = b(w, 15, 13);
            if option & 2 == 0 {
                return None;
            }
            let extend = Extend::from_option(option);
            (LsFlavour::Normal, Addr::RegOffset { rn, rm: reg(w, 16), extend, shifted: bit(w, 12) })
        }
    };
    Some(Insn::LoadStore { kind, flavour, size: Size(size as u8), rt: reg(w, 0), addr })
}

fn atomic(w: u32) -> Option<Insn> {
    if bit(w, 26) {
        return None;
    }
    let size = Size(b(w, 31, 30) as u8);
    let (acquire, release) = (bit(w, 23), bit(w, 22));
    let (rs, rn, rt) = (reg(w, 16), reg(w, 5), reg(w, 0));
    let opc = b(w, 14, 12);
    let op = if !bit(w, 15) {
        [
            AtomicOp::Add,
            AtomicOp::Clr,
            AtomicOp::Eor,
            AtomicOp::Set,
            AtomicOp::Smax,
            AtomicOp::Smin,
            AtomicOp::Umax,
            AtomicOp::Umin,
        ][opc as usize]
    } else if opc == 0 {
        AtomicOp::Swp
    } else if opc == 0b100 && acquire && !release {
        // (LDAPR: Rs should be one)
        return Some(Insn::Ldapr { size, rt, rn });
    } else {
        return None;
    };
    Some(Insn::Atomic { op, size, acquire, release, rs, rt, rn })
}

fn ldr_pac(w: u32) -> Option<Insn> {
    if b(w, 31, 30) != 3 || bit(w, 26) {
        return None;
    }
    let imm = sext(((bit(w, 22) as u32) << 9) | b(w, 20, 12), 10) * 8;
    Some(Insn::LdrPac { b: bit(w, 23), writeback: bit(w, 11), rt: reg(w, 0), rn: reg(w, 5), imm })
}

// ---- data processing, register

fn dp_reg(w: u32) -> Option<Insn> {
    let sf = bit(w, 31);
    let (rd, rn, rm) = (reg(w, 0), reg(w, 5), reg(w, 16));
    let op2 = b(w, 24, 21);
    let op3 = b(w, 15, 10);
    if !bit(w, 28) {
        let amount = b(w, 15, 10) as u8;
        if op2 >> 3 == 0 {
            if !sf && amount >= 32 {
                return None;
            }
            let shift = [Shift::Lsl, Shift::Lsr, Shift::Asr, Shift::Ror][b(w, 23, 22) as usize];
            let op = [
                LogicOp::And,
                LogicOp::Bic,
                LogicOp::Orr,
                LogicOp::Orn,
                LogicOp::Eor,
                LogicOp::Eon,
                LogicOp::Ands,
                LogicOp::Bics,
            ][(b(w, 30, 29) * 2 + bit(w, 21) as u32) as usize];
            return Some(Insn::LogicalReg { sf, op, rd, rn, rm, shift, amount });
        }
        let (sub, setflags) = (bit(w, 30), bit(w, 29));
        if op2 & 1 == 0 {
            let shift = match b(w, 23, 22) {
                0 => Shift::Lsl,
                1 => Shift::Lsr,
                2 => Shift::Asr,
                _ => return None,
            };
            if !sf && amount >= 32 {
                return None;
            }
            return Some(Insn::AddSubReg { sf, sub, setflags, rd, rn, rm, shift, amount });
        }
        let imm3 = b(w, 12, 10) as u8;
        if b(w, 23, 22) != 0 || imm3 > 4 {
            return None;
        }
        let extend = Extend::from_option(b(w, 15, 13));
        return Some(Insn::AddSubExt { sf, sub, setflags, rd, rn, rm, extend, amount: imm3 });
    }
    match op2 {
        0b0000 => match op3 {
            0 => Some(Insn::AddSubCarry { sf, sub: bit(w, 30), setflags: bit(w, 29), rd, rn, rm }),
            _ if op3 & 0b011111 == 0b000001 => {
                if b(w, 31, 29) != 0b101 || bit(w, 4) {
                    return None;
                }
                Some(Insn::Rmif { rn, shift: b(w, 20, 15) as u8, mask: b(w, 3, 0) as u8 })
            }
            _ if op3 & 0b001111 == 0b000010 => {
                if b(w, 31, 29) != 0b001 || b(w, 20, 15) != 0 || bit(w, 4) || b(w, 3, 0) != 0b1101 {
                    return None;
                }
                Some(Insn::Setf { sixteen: bit(w, 14), rn })
            }
            _ => None,
        },
        0b0010 => {
            if !bit(w, 29) || bit(w, 10) || bit(w, 4) {
                return None;
            }
            let rhs = if bit(w, 11) { CmpRhs::Imm(b(w, 20, 16) as u8) } else { CmpRhs::Reg(rm) };
            Some(Insn::CondCompare {
                sf,
                negate: !bit(w, 30),
                rn,
                rhs,
                nzcv: b(w, 3, 0) as u8,
                cond: Cond(b(w, 15, 12) as u8),
            })
        }
        0b0100 => {
            if bit(w, 29) {
                return None;
            }
            let op = match (bit(w, 30), b(w, 11, 10)) {
                (false, 0) => CselOp::Csel,
                (false, 1) => CselOp::Csinc,
                (true, 0) => CselOp::Csinv,
                (true, 1) => CselOp::Csneg,
                _ => return None,
            };
            Some(Insn::CondSelect { sf, op, rd, rn, rm, cond: Cond(b(w, 15, 12) as u8) })
        }
        0b0110 => {
            if bit(w, 29) {
                return None;
            }
            if bit(w, 30) { dp1(w, sf, rd, rn) } else { dp2(w, sf, rd, rn, rm) }
        }
        _ if op2 >> 3 == 1 => dp3(w, sf, rd, rn, rm),
        _ => None,
    }
}

fn dp2(w: u32, sf: bool, rd: Reg, rn: Reg, rm: Reg) -> Option<Insn> {
    let op = match b(w, 15, 10) {
        0b000010 => DataProc2Op::Udiv,
        0b000011 => DataProc2Op::Sdiv,
        0b001000 => DataProc2Op::Lslv,
        0b001001 => DataProc2Op::Lsrv,
        0b001010 => DataProc2Op::Asrv,
        0b001011 => DataProc2Op::Rorv,
        0b001100 if sf => DataProc2Op::Pacga,
        o @ 0b010000..=0b010111 => {
            let size = o & 3;
            if sf != (size == 3) {
                return None;
            }
            return Some(Insn::Crc32 { castagnoli: o & 4 != 0, size: Size(size as u8), rd, rn, rm });
        }
        _ => return None,
    };
    Some(Insn::DataProc2 { sf, op, rd, rn, rm })
}

fn dp1(w: u32, sf: bool, rd: Reg, rn: Reg) -> Option<Insn> {
    let opcode = b(w, 15, 10);
    match b(w, 20, 16) {
        0b00000 => {
            let op = match opcode {
                0b000000 => DataProc1Op::Rbit,
                0b000001 => DataProc1Op::Rev16,
                0b000010 => {
                    if sf {
                        DataProc1Op::Rev32
                    } else {
                        DataProc1Op::Rev
                    }
                }
                0b000011 if sf => DataProc1Op::Rev,
                0b000100 => DataProc1Op::Clz,
                0b000101 => DataProc1Op::Cls,
                _ => return None,
            };
            Some(Insn::DataProc1 { sf, op, rd, rn })
        }
        0b00001 if sf => {
            let key = [PacKey::Ia, PacKey::Ib, PacKey::Da, PacKey::Db][(opcode & 3) as usize];
            match opcode {
                0b000000..=0b000111 => Some(Insn::Pac { op: PacOp { auth: opcode & 4 != 0, key }, rd, rn: Some(rn) }),
                0b001000..=0b001111 if rn.is_31() => {
                    Some(Insn::Pac { op: PacOp { auth: opcode & 4 != 0, key }, rd, rn: None })
                }
                0b010000 | 0b010001 if rn.is_31() => Some(Insn::Xpac { data: opcode & 1 != 0, rd }),
                _ => None,
            }
        }
        _ => None,
    }
}

fn dp3(w: u32, sf: bool, rd: Reg, rn: Reg, rm: Reg) -> Option<Insn> {
    if b(w, 30, 29) != 0 {
        return None;
    }
    let ra = reg(w, 10);
    let op = match (b(w, 23, 21), bit(w, 15)) {
        (0b000, false) => DataProc3Op::Madd,
        (0b000, true) => DataProc3Op::Msub,
        (0b001, false) if sf => DataProc3Op::Smaddl,
        (0b001, true) if sf => DataProc3Op::Smsubl,
        (0b010, false) if sf => DataProc3Op::Smulh, // (Ra should be one)
        (0b101, false) if sf => DataProc3Op::Umaddl,
        (0b101, true) if sf => DataProc3Op::Umsubl,
        (0b110, false) if sf => DataProc3Op::Umulh,
        _ => return None,
    };
    Some(Insn::DataProc3 { sf, op, rd, rn, rm, ra })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What discovery reads of each branch kind (encodings from llvm-mc).
    #[test]
    fn branch_flow() {
        let cases: &[(u32, Flow)] = &[
            (0x14000002, Flow::Jump { offset: 8 }),       // b #8
            (0x97ffffff, Flow::Call { offset: -4 }),      // bl #-4
            (0x54ffffc1, Flow::CondJump { offset: -8 }),  // b.ne #-8
            (0x34000083, Flow::CondJump { offset: 16 }),  // cbz w3, #16
            (0xb747ff05, Flow::CondJump { offset: -32 }), // tbnz x5, #40, #-32
            (0xd61f0200, Flow::JumpReg),                  // br x16
            (0xd63f0100, Flow::CallReg),                  // blr x8
            (0xd65f03c0, Flow::Return),                   // ret
            (0xd65f0060, Flow::Return),                   // ret x3
            (0xd65f0bff, Flow::Return),                   // retaa
            (0xd61f083f, Flow::JumpReg),                  // braaz x1
            (0xd73f0822, Flow::CallReg),                  // blraa x1, x2
            (0xd4001001, Flow::Svc),                      // svc #0x80
            (0xd4200020, Flow::Trap),                     // brk #1
            (0x00000000, Flow::Trap),                     // udf #0
            (0xd69f03e0, Flow::Trap),                     // eret: undefined at EL0
            (0xd69f0bff, Flow::Trap),                     // eretaa
            (0xd6bf03e0, Flow::Trap),                     // drps
            (0x8b020020, Flow::Next),                     // add x0, x1, x2
        ];
        for &(w, flow) in cases {
            assert_eq!(decode(w).map(|i| i.flow()), Some(flow), "{w:#010x}");
        }
    }

    #[test]
    fn fields() {
        // tbnz x5, #40, #-32
        assert_eq!(decode(0xb747ff05), Some(Insn::Tbz { nonzero: true, rt: Reg(5), bit: 40, offset: -32 }));
        // retaa: RET of x30, authenticated with the A key and SP
        assert_eq!(
            decode(0xd65f0bff),
            Some(Insn::BranchReg { op: BranchRegOp::Ret, rn: Reg(30), auth: BranchAuth::Sp { b: false } })
        );
        // mrs x0, tpidr_el0
        assert_eq!(decode(0xd53bd040), Some(Insn::Mrs { reg: SysReg::TPIDR_EL0, rt: Reg(0) }));
        // ldr q0, [sp], #16
        assert_eq!(
            decode(0x3cc107e0),
            Some(Insn::LoadStore {
                kind: LsKind::LoadFp,
                flavour: LsFlavour::Normal,
                size: Size(4),
                rt: Reg(0),
                addr: Addr::PostIndex { rn: Reg(31), imm: 16 },
            })
        );
        // ldr x0, #8
        assert_eq!(
            decode(0x58000040),
            Some(Insn::LoadStore {
                kind: LsKind::Load,
                flavour: LsFlavour::Normal,
                size: Size(3),
                rt: Reg(0),
                addr: Addr::Literal { imm: 8 },
            })
        );
        // swpal w0, w1, [sp]
        assert_eq!(
            decode(0xb8e083e1),
            Some(Insn::Atomic {
                op: AtomicOp::Swp,
                size: Size(2),
                acquire: true,
                release: true,
                rs: Reg(0),
                rt: Reg(1),
                rn: Reg(31),
            })
        );
    }
}
