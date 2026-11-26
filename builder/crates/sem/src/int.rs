//! Data processing (immediate and register): the Arm ARM's pseudocode for each form.

use crate::{Fault, Sem};
use a64::{
    BitfieldOp, CmpRhs, CselOp, DataProc1Op, DataProc2Op, DataProc3Op, Extend, Insn, LogicOp, MoveOp, Reg, Shift,
};
use mir::{BinOp, Flag, Pred, Type, UnOp, Value};

/// The width of a form: X (`sf`) or W.
fn width(sf: bool) -> Type {
    if sf { Type::I64 } else { Type::I32 }
}

/// DecodeBitMasks(N, imms, immr, immediate = false): (wmask, tmask), the bitfield moves' masks, of `bits` bits.
fn bitfield_masks(n: bool, imms: u32, immr: u32, bits: u32) -> (u128, u128) {
    let combined = (u32::from(n) << 6) | (!imms & 0x3f);
    let len = 31 - combined.leading_zeros(); // (the decoder refused the fields with no length)
    let levels = (1u32 << len) - 1;
    let s = imms & levels;
    let r = immr & levels;
    let esize = 1u32 << len;
    let d = s.wrapping_sub(r) & (esize - 1);
    let ones = |k: u32| -> u128 { if k >= 128 { u128::MAX } else { (1u128 << k) - 1 } };
    let emask = ones(esize);
    let welem = ones(s + 1);
    let wrot = if r == 0 { welem } else { ((welem >> r) | (welem << (esize - r))) & emask };
    let telem = ones(d + 1);
    let replicate = |e: u128| (0..bits / esize).fold(0u128, |acc, i| acc | (e << (i * esize)));
    (replicate(wrot), replicate(telem))
}

impl Sem<'_> {
    pub(crate) fn data(&mut self, insn: &Insn) -> Result<(), Fault> {
        match *insn {
            Insn::Adr { page, rd, imm } => {
                let base = if page { self.pc() & !0xfff } else { self.pc() };
                let v = self.k64(base.wrapping_add(imm as u64));
                self.write(rd, v, false);
            }
            Insn::AddSubImm { sf, sub, setflags, rd, rn, imm12, shift12 } => {
                let a = self.read(rn, sf, true);
                let b = self.konst(width(sf), u128::from(imm12) << if shift12 { 12 } else { 0 });
                let r = self.add_sub(a, b, sub, setflags);
                self.write(rd, r, !setflags);
            }
            Insn::LogicalImm { sf, op, rd, rn, imm } => {
                let a = self.read(rn, sf, false);
                let b = self.konst(width(sf), u128::from(imm));
                let r = self.logic(op, a, b);
                self.write(rd, r, op != LogicOp::Ands);
            }
            Insn::MoveWide { sf, op, rd, imm16, hw } => {
                let ty = width(sf);
                let shift = u32::from(hw) * 16;
                let field = u128::from(imm16) << shift;
                let r = match op {
                    MoveOp::Movz => self.konst(ty, field),
                    MoveOp::Movn => self.konst(ty, !field),
                    MoveOp::Movk => {
                        let old = self.read(rd, sf, false);
                        let kept = self.and_k(old, !(0xffffu128 << shift));
                        let k = self.konst(ty, field);
                        self.bin(BinOp::Or, kept, k)
                    }
                };
                self.write(rd, r, false);
            }
            Insn::Bitfield { sf, op, rd, rn, immr, imms } => {
                let r = self.bitfield(sf, op, rd, rn, u32::from(immr), u32::from(imms));
                self.write(rd, r, false);
            }
            Insn::Extr { sf, rd, rn, rm, lsb } => {
                let lo = self.read(rm, sf, false);
                let r = if lsb == 0 {
                    lo
                } else {
                    let hi = self.read(rn, sf, false);
                    let lo = self.lshr_k(lo, u32::from(lsb));
                    let hi = self.shl_k(hi, width(sf).bits() - u32::from(lsb));
                    self.bin(BinOp::Or, hi, lo)
                };
                self.write(rd, r, false);
            }
            Insn::LogicalReg { sf, op, rd, rn, rm, shift, amount } => {
                let a = self.read(rn, sf, false);
                let b = self.read(rm, sf, false);
                let b = self.shift(b, shift, u32::from(amount));
                let r = self.logic(op, a, b);
                self.write(rd, r, false);
            }
            Insn::AddSubReg { sf, sub, setflags, rd, rn, rm, shift, amount } => {
                let a = self.read(rn, sf, false);
                let b = self.read(rm, sf, false);
                let b = self.shift(b, shift, u32::from(amount));
                let r = self.add_sub(a, b, sub, setflags);
                self.write(rd, r, false);
            }
            Insn::AddSubExt { sf, sub, setflags, rd, rn, rm, extend, amount } => {
                let a = self.read(rn, sf, true);
                let b = self.extend_reg(rm, extend, u32::from(amount), width(sf));
                let r = self.add_sub(a, b, sub, setflags);
                self.write(rd, r, !setflags);
            }
            Insn::AddSubCarry { sf, sub, setflags, rd, rn, rm } => {
                let a = self.read(rn, sf, false);
                let b = self.read(rm, sf, false);
                let b = if sub { self.un(UnOp::Not, b) } else { b };
                let carry = self.flag(Flag::C);
                let r = self.add_with_carry(a, b, carry, setflags);
                self.write(rd, r, false);
            }
            Insn::Rmif { rn, shift, mask } => {
                let x = self.x(rn);
                let rotated = self.ror_k(x, u32::from(shift));
                for (bit, flag) in [(3, Flag::N), (2, Flag::Z), (1, Flag::C), (0, Flag::V)] {
                    if mask >> bit & 1 == 1 {
                        let b = self.bit(rotated, bit);
                        self.set(mir::Loc::Flag(flag), b);
                    }
                }
            }
            Insn::Setf { sixteen, rn } => {
                let top = if sixteen { 15 } else { 7 };
                let x = self.read(rn, false, false);
                let n = self.bit(x, top);
                let low = self.and_k(x, (1u128 << (top + 1)) - 1);
                let z = self.is_zero(low);
                let above = self.bit(x, top + 1);
                let v = self.bin(BinOp::Xor, above, n);
                self.set(mir::Loc::Flag(Flag::N), n);
                self.set(mir::Loc::Flag(Flag::Z), z);
                self.set(mir::Loc::Flag(Flag::V), v);
            }
            Insn::CondCompare { sf, negate, rn, rhs, nzcv, cond } => {
                let holds = self.cond(cond);
                let a = self.read(rn, sf, false);
                let b = match rhs {
                    CmpRhs::Reg(r) => self.read(r, sf, false),
                    CmpRhs::Imm(i) => self.konst(width(sf), u128::from(i)),
                };
                let (n, z, c, v) = self.add_sub_flags(a, b, !negate);
                let mut pick = |flag_bit: u8, computed: Value| {
                    let given = self.konst(Type::I1, u128::from(nzcv >> flag_bit & 1));
                    self.select(holds, computed, given)
                };
                let (n, z, c, v) = (pick(3, n), pick(2, z), pick(1, c), pick(0, v));
                self.set_nzcv(n, z, c, v);
            }
            Insn::CondSelect { sf, op, rd, rn, rm, cond } => {
                let holds = self.cond(cond);
                let a = self.read(rn, sf, false);
                let b = self.read(rm, sf, false);
                let b = match op {
                    CselOp::Csel => b,
                    CselOp::Csinc => {
                        let one = self.k_like(b, 1);
                        self.bin(BinOp::Add, b, one)
                    }
                    CselOp::Csinv => self.un(UnOp::Not, b),
                    CselOp::Csneg => self.un(UnOp::Neg, b),
                };
                let r = self.select(holds, a, b);
                self.write(rd, r, false);
            }
            Insn::DataProc1 { sf, op, rd, rn } => {
                let x = self.read(rn, sf, false);
                let r = self.data1(op, x);
                self.write(rd, r, false);
            }
            Insn::DataProc2 { sf, op, rd, rn, rm } => {
                let r = if op == DataProc2Op::Pacga {
                    self.k64(0) // (the host's model: no PAC, so its code is 0)
                } else {
                    let a = self.read(rn, sf, false);
                    let b = self.read(rm, sf, false);
                    self.data2(op, a, b)
                };
                self.write(rd, r, false);
            }
            Insn::Crc32 { castagnoli, size, rd, rn, rm } => {
                let crc = self.read(rn, false, false);
                let data = self.read(rm, size.0 == 3, false);
                let r = self.crc32(crc, data, size.bytes() * 8, castagnoli);
                self.write(rd, r, false);
            }
            // (pointer authentication is the identity: sign, authenticate and strip change nothing)
            Insn::Pac { .. } | Insn::Xpac { .. } => {}
            Insn::DataProc3 { sf, op, rd, rn, rm, ra } => {
                let r = self.data3(sf, op, rn, rm, ra);
                self.write(rd, r, false);
            }
            _ => unreachable!("not data processing: {insn}"),
        }
        Ok(())
    }

    /// AddWithCarry(a, b, carry): the result, and the flags set when `setflags`.
    fn add_with_carry(&mut self, a: Value, b: Value, carry: Value, setflags: bool) -> Value {
        let ty = self.ty(a);
        let c = self.zext(carry, ty);
        let ab = self.bin(BinOp::Add, a, b);
        let r = self.bin(BinOp::Add, ab, c);
        if setflags {
            let wide = Type::int(ty.bits() * 2).expect("a 32- or 64-bit operand");
            let (wa, wb, wc) = (self.zext(a, wide), self.zext(b, wide), self.zext(carry, wide));
            let sum = self.bin(BinOp::Add, wa, wb);
            let sum = self.bin(BinOp::Add, sum, wc);
            let high = self.lshr_k(sum, ty.bits());
            let c_out = self.zext(high, Type::I1);
            let v = self.overflow(a, b, r);
            let n = self.is_negative(r);
            let z = self.is_zero(r);
            self.set_nzcv(n, z, c_out, v);
        }
        r
    }

    /// V of `a + b = r` (b already inverted for a subtraction): the operands had one sign and the result the other.
    fn overflow(&mut self, a: Value, b: Value, r: Value) -> Value {
        let ar = self.bin(BinOp::Xor, a, r);
        let br = self.bin(BinOp::Xor, b, r);
        let both = self.bin(BinOp::And, ar, br);
        self.is_negative(both)
    }

    /// The flags of `a + b` (`sub` false) or `a - b`, without the result.
    fn add_sub_flags(&mut self, a: Value, b: Value, sub: bool) -> (Value, Value, Value, Value) {
        let (r, c) = if sub {
            (self.bin(BinOp::Sub, a, b), self.cmp(Pred::Uge, a, b))
        } else {
            let r = self.bin(BinOp::Add, a, b);
            (r, self.cmp(Pred::Ult, r, a))
        };
        let b = if sub { self.un(UnOp::Not, b) } else { b };
        let v = self.overflow(a, b, r);
        let n = self.is_negative(r);
        let z = self.is_zero(r);
        (n, z, c, v)
    }

    /// ADD, ADDS, SUB, SUBS: `a + b` or `a - b`, the flags set when `setflags`.
    fn add_sub(&mut self, a: Value, b: Value, sub: bool, setflags: bool) -> Value {
        if setflags {
            let (n, z, c, v) = self.add_sub_flags(a, b, sub);
            self.set_nzcv(n, z, c, v);
        }
        self.bin(if sub { BinOp::Sub } else { BinOp::Add }, a, b)
    }

    /// A logical operation; ANDS and BICS set N and Z, and clear C and V.
    fn logic(&mut self, op: LogicOp, a: Value, b: Value) -> Value {
        let inverted = matches!(op, LogicOp::Bic | LogicOp::Orn | LogicOp::Eon | LogicOp::Bics);
        let b = if inverted { self.un(UnOp::Not, b) } else { b };
        let r = match op {
            LogicOp::And | LogicOp::Bic | LogicOp::Ands | LogicOp::Bics => self.bin(BinOp::And, a, b),
            LogicOp::Orr | LogicOp::Orn => self.bin(BinOp::Or, a, b),
            LogicOp::Eor | LogicOp::Eon => self.bin(BinOp::Xor, a, b),
        };
        if matches!(op, LogicOp::Ands | LogicOp::Bics) {
            let n = self.is_negative(r);
            let z = self.is_zero(r);
            let zero = self.konst(Type::I1, 0);
            self.set_nzcv(n, z, zero, zero);
        }
        r
    }

    /// ShiftReg: `v` shifted by the constant `amount` (less than its width).
    fn shift(&mut self, v: Value, shift: Shift, amount: u32) -> Value {
        match shift {
            Shift::Lsl => self.shl_k(v, amount),
            Shift::Lsr => self.lshr_k(v, amount),
            Shift::Asr => self.ashr_k(v, amount),
            Shift::Ror => self.ror_k(v, amount),
        }
    }

    /// ExtendReg: register `r`'s low bits extended to `ty` and shifted left by `amount`.
    fn extend_reg(&mut self, r: Reg, extend: Extend, amount: u32, ty: Type) -> Value {
        let x = self.x(r);
        let (bits, signed) = match extend {
            Extend::Uxtb => (8, false),
            Extend::Uxth => (16, false),
            Extend::Uxtw => (32, false),
            Extend::Uxtx => (64, false),
            Extend::Sxtb => (8, true),
            Extend::Sxth => (16, true),
            Extend::Sxtw => (32, true),
            Extend::Sxtx => (64, true),
        };
        // (the extend's low bits, then to the operation's width: widened by the extend's kind, or cut to it)
        let part = self.zext(x, Type::int(bits).expect("an extend's width"));
        let v = self.resize(part, ty, signed);
        self.shl_k(v, amount)
    }

    /// SBFM, BFM, UBFM (the Arm ARM's: rotate, then mask, with the sign or the destination's bits above).
    fn bitfield(&mut self, sf: bool, op: BitfieldOp, rd: Reg, rn: Reg, immr: u32, imms: u32) -> Value {
        let ty = width(sf);
        let bits = ty.bits();
        let (wmask, tmask) = bitfield_masks(sf, imms, immr, bits);
        let src = self.read(rn, sf, false);
        let rotated = self.ror_k(src, immr);
        let bot = match op {
            BitfieldOp::Bfm => {
                let dst = self.read(rd, sf, false);
                let kept = self.and_k(dst, !wmask);
                let moved = self.and_k(rotated, wmask);
                let bot = self.bin(BinOp::Or, kept, moved);
                let above = self.and_k(dst, !tmask);
                let bot = self.and_k(bot, tmask);
                return self.bin(BinOp::Or, above, bot);
            }
            _ => self.and_k(rotated, wmask),
        };
        let bot = self.and_k(bot, tmask);
        if op == BitfieldOp::Ubfm {
            return bot;
        }
        // (SBFM: every bit above the field is the field's top bit, src<imms>)
        let up = self.shl_k(src, bits - 1 - imms);
        let top = self.ashr_k(up, bits - 1);
        let top = self.and_k(top, !tmask);
        self.bin(BinOp::Or, top, bot)
    }

    fn data1(&mut self, op: DataProc1Op, x: Value) -> Value {
        let bits = self.ty(x).bits();
        match op {
            DataProc1Op::Rbit => self.un(UnOp::BitReverse, x),
            DataProc1Op::Rev => self.un(UnOp::ByteSwap, x),
            DataProc1Op::Rev16 => {
                let m = (0..bits / 16).fold(0u128, |acc, i| acc | (0x00ffu128 << (16 * i)));
                let lo = self.lshr_k(x, 8);
                let lo = self.and_k(lo, m);
                let hi = self.and_k(x, m);
                let hi = self.shl_k(hi, 8);
                self.bin(BinOp::Or, lo, hi)
            }
            DataProc1Op::Rev32 => {
                let swapped = self.un(UnOp::ByteSwap, x);
                self.ror_k(swapped, 32)
            }
            DataProc1Op::Clz => self.un(UnOp::Ctlz, x),
            DataProc1Op::Cls => {
                // (CLS(x) = CLZ(x XOR (x ASR 1)) - 1: the bits after the top one that equal it)
                let half = self.ashr_k(x, 1);
                let diff = self.bin(BinOp::Xor, x, half);
                let lz = self.un(UnOp::Ctlz, diff);
                let one = self.k_like(x, 1);
                self.bin(BinOp::Sub, lz, one)
            }
        }
    }

    fn data2(&mut self, op: DataProc2Op, a: Value, b: Value) -> Value {
        let ty = self.ty(a);
        let bits = ty.bits();
        match op {
            DataProc2Op::Udiv => {
                // (a division by zero is 0)
                let zero = self.is_zero(b);
                let one = self.k_like(b, 1);
                let safe = self.select(zero, one, b);
                let q = self.bin(BinOp::UDiv, a, safe);
                let z = self.k_like(a, 0);
                self.select(zero, z, q)
            }
            DataProc2Op::Sdiv => {
                // (by zero: 0; MIN / -1: MIN, the wrapped quotient)
                let zero = self.is_zero(b);
                let min = self.k_like(a, 1u128 << (bits - 1));
                let minus_one = self.k_like(b, ty.mask());
                let a_min = self.cmp(Pred::Eq, a, min);
                let b_m1 = self.cmp(Pred::Eq, b, minus_one);
                let wraps = self.bin(BinOp::And, a_min, b_m1);
                let unsafe_ = self.bin(BinOp::Or, zero, wraps);
                let one = self.k_like(b, 1);
                let safe = self.select(unsafe_, one, b);
                let q = self.bin(BinOp::SDiv, a, safe);
                let q = self.select(wraps, min, q);
                let z = self.k_like(a, 0);
                self.select(zero, z, q)
            }
            DataProc2Op::Lslv | DataProc2Op::Lsrv | DataProc2Op::Asrv => {
                let n = self.and_k(b, u128::from(bits - 1));
                let op = match op {
                    DataProc2Op::Lslv => BinOp::Shl,
                    DataProc2Op::Lsrv => BinOp::LShr,
                    _ => BinOp::AShr,
                };
                self.bin(op, a, n)
            }
            DataProc2Op::Rorv => {
                let n = self.and_k(b, u128::from(bits - 1));
                let lo = self.bin(BinOp::LShr, a, n);
                let w = self.k_like(b, u128::from(bits));
                let back = self.bin(BinOp::Sub, w, n);
                let back = self.and_k(back, u128::from(bits - 1));
                let hi = self.bin(BinOp::Shl, a, back);
                self.bin(BinOp::Or, lo, hi)
            }
            DataProc2Op::Pacga => unreachable!("lowered by data()"),
        }
    }

    /// CRC32*: the reflected CRC of `bits` bits of `data` on `crc`, bit by bit (CRC-32 or, `castagnoli`, CRC-32C).
    fn crc32(&mut self, crc: Value, data: Value, bits: u32, castagnoli: bool) -> Value {
        let poly: u128 = if castagnoli { 0x82f6_3b78 } else { 0xedb8_8320 };
        let low = self.zext(data, Type::I32);
        let mut crc = if bits >= 32 {
            self.bin(BinOp::Xor, crc, low)
        } else {
            let part = self.and_k(low, (1u128 << bits) - 1);
            self.bin(BinOp::Xor, crc, part)
        };
        for i in 0..bits {
            if i == 32 {
                let high = self.lshr_k(data, 32);
                let high = self.zext(high, Type::I32);
                crc = self.bin(BinOp::Xor, crc, high);
            }
            let lsb = self.and_k(crc, 1);
            let mask = self.un(UnOp::Neg, lsb);
            let p = self.and_k(mask, poly);
            let shifted = self.lshr_k(crc, 1);
            crc = self.bin(BinOp::Xor, shifted, p);
        }
        crc
    }

    fn data3(&mut self, sf: bool, op: DataProc3Op, rn: Reg, rm: Reg, ra: Reg) -> Value {
        match op {
            DataProc3Op::Madd | DataProc3Op::Msub => {
                let a = self.read(rn, sf, false);
                let b = self.read(rm, sf, false);
                let acc = self.read(ra, sf, false);
                let p = self.bin(BinOp::Mul, a, b);
                self.bin(if op == DataProc3Op::Madd { BinOp::Add } else { BinOp::Sub }, acc, p)
            }
            DataProc3Op::Smaddl | DataProc3Op::Smsubl | DataProc3Op::Umaddl | DataProc3Op::Umsubl => {
                let signed = matches!(op, DataProc3Op::Smaddl | DataProc3Op::Smsubl);
                let a = self.read(rn, false, false);
                let b = self.read(rm, false, false);
                let a = self.resize(a, Type::I64, signed);
                let b = self.resize(b, Type::I64, signed);
                let acc = self.x(ra);
                let p = self.bin(BinOp::Mul, a, b);
                let add = matches!(op, DataProc3Op::Smaddl | DataProc3Op::Umaddl);
                self.bin(if add { BinOp::Add } else { BinOp::Sub }, acc, p)
            }
            DataProc3Op::Smulh | DataProc3Op::Umulh => {
                let signed = op == DataProc3Op::Smulh;
                let a = self.x(rn);
                let b = self.x(rm);
                let a = self.resize(a, Type::I128, signed);
                let b = self.resize(b, Type::I128, signed);
                let p = self.bin(BinOp::Mul, a, b);
                let high = self.lshr_k(p, 64);
                self.zext(high, Type::I64)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bitfield_masks;

    #[test]
    fn bitfield_masks_match_the_arm_arm() {
        // ubfx w0, w1, #3, #5 = UBFM immr=3 imms=7: wmask rotates 0xff right by 3, tmask is 5 ones
        let (w, t) = bitfield_masks(false, 7, 3, 32);
        assert_eq!(w, 0xe000_001f);
        assert_eq!(t, 0x1f);
        // lsl w0, w1, #1 = UBFM immr=31 imms=30: wmask 0x7fffffff rotated right 31 = 0xfffffffe, tmask all ones
        let (w, t) = bitfield_masks(false, 30, 31, 32);
        assert_eq!(w, 0xffff_fffe);
        assert_eq!(t, 0xffff_ffff);
        // sxtw x0, w1 = SBFM immr=0 imms=31 (64-bit)
        let (w, t) = bitfield_masks(true, 31, 0, 64);
        assert_eq!(w, 0xffff_ffff);
        assert_eq!(t, 0xffff_ffff);
    }
}
