//! The Advanced SIMD data-processing instructions, lane by lane on LLVM vectors (a scalar form is the same operation on
//! one element). Integer saturation sets FPSR.QC, as the architecture does.

use crate::fpfast::Fast;
use crate::fpscalar::{float_elem, rounding};
use crate::vreg::{int_type, shape_float, shape_int, vector_of};
use crate::{Fault, Sem};
use a64::{
    AcrossOp, Arr, CmpZero, DiffForm, DiffOp, ElemOp, Insn, ModImmOp, PairwiseOp, PermuteOp, Shape, ShiftOp, Simd2Op,
    Simd3Op, Size, VReg,
};
use mir::{BinOp, Cast, Elem, FBinOp, FPred, FUnOp, Loc, Op, Pred, Reduce, Table, Type, UnOp, Value};

/// FPSR.QC, the cumulative saturation flag.
const QC: u128 = 1 << 27;

/// A shape's upper-half flag: the "2" forms read or write the upper 64 bits of a register.
fn upper(shape: Shape) -> bool {
    matches!(shape, Shape::Vector(Arr { q: true, .. }))
}

/// The 64-bit (or scalar) narrow operand type of a widening or narrowing form's shape.
fn narrow_type(shape: Shape) -> Type {
    match shape {
        Shape::Vector(a) => vector_of(Arr::new(a.size, false), int_type(a.size)),
        Shape::Scalar(s) => int_type(s),
    }
}

/// The wide operand type of a widening or narrowing form: twice the narrow elements, as many of them.
fn wide_type(shape: Shape) -> Type {
    let n = narrow_type(shape);
    n.with_elem(int_type(Size(shape.size().0 + 1)))
}

/// The bytes SDOT and UDOT of `shape` (2S or 4S) take their products of: 8B or 16B.
fn dot_bytes(shape: Shape) -> Type {
    vector_of(Arr::new(Size(0), upper(shape)), Type::I8)
}

/// What a widening multiply does with its product: nothing, add it to the destination or take it away, and the
/// same of twice the product, saturated (SQDMULL ...).
#[derive(Clone, Copy)]
enum WideMul {
    Mul,
    Acc,
    Sub,
    QMul,
    QAcc,
    QSub,
}

/// The signed minimum and maximum of `bits`-bit elements, as bit patterns of `bits` bits.
fn smin(bits: u32) -> u128 {
    1u128 << (bits - 1)
}
fn smax(bits: u32) -> u128 {
    (1u128 << (bits - 1)) - 1
}
fn umax(bits: u32) -> u128 {
    if bits >= 128 { u128::MAX } else { (1u128 << bits) - 1 }
}

impl Sem<'_> {
    pub(crate) fn simd(&mut self, insn: &Insn) -> Result<(), Fault> {
        match *insn {
            Insn::Simd3 { op, shape, rd, rn, rm } => self.simd3(op, shape, rd, rn, rm),
            Insn::SimdDiff { op, shape, rd, rn, rm } => self.simd_diff(op, shape, rd, rn, rm),
            Insn::Simd2 { op, shape, rd, rn } => self.simd2(op, shape, rd, rn),
            Insn::SimdAcross { op, arr, rd, rn } => self.across(op, arr, rd, rn),
            Insn::SimdPairwise { op, size, rd, rn } => {
                let elem = if op == PairwiseOp::Addp {
                    Elem::I64
                } else {
                    Elem::of(float_elem(size.bytes() * 8, true)?).expect("a float element")
                };
                let ty = Type::Vec(2, elem);
                let v = self.vread(rn, ty);
                let a = self.op(Op::Extract(v, 0));
                let b = self.op(Op::Extract(v, 1));
                let r = match op {
                    PairwiseOp::Addp => self.bin(BinOp::Add, a, b),
                    PairwiseOp::Faddp => self.fp(Fast::Arith(FBinOp::Add), &[a, b]),
                    PairwiseOp::Fmaxp | PairwiseOp::Fminp | PairwiseOp::Fmaxnmp | PairwiseOp::Fminnmp => {
                        let max = matches!(op, PairwiseOp::Fmaxp | PairwiseOp::Fmaxnmp);
                        let nm = matches!(op, PairwiseOp::Fmaxnmp | PairwiseOp::Fminnmp);
                        self.fp(Fast::MinMax { max, nm }, &[a, b])
                    }
                };
                self.vwrite(rd, r);
                Ok(())
            }
            Insn::SimdShift { op, shape, rd, rn, shift } => self.shift_imm(op, shape, rd, rn, u32::from(shift)),
            Insn::SimdElem { op, shape, rd, rn, rm, index } => self.by_element(op, shape, rd, rn, rm, index),
            Insn::SimdPermute { op, second, arr, rd, rn, rm } => {
                let ty = vector_of(arr, int_type(arr.size));
                let (n, m) = (self.vread(rn, ty), self.vread(rm, ty));
                let l = arr.lanes() as u8;
                let s = u8::from(second);
                let mask = (0..l)
                    .map(|i| match op {
                        PermuteOp::Uzp => 2 * i + s,
                        PermuteOp::Trn if i % 2 == 0 => i + s,
                        PermuteOp::Trn => l + i - 1 + s,
                        PermuteOp::Zip => (i % 2) * l + s * (l / 2) + i / 2,
                    })
                    .collect();
                let r = self.op(Op::Shuffle(n, m, mask));
                self.vwrite(rd, r);
                Ok(())
            }
            Insn::SimdExt { q, rd, rn, rm, index } => {
                let l = if q { 16 } else { 8 };
                let ty = Type::Vec(l, Elem::I8);
                let (n, m) = (self.vread(rn, ty), self.vread(rm, ty));
                let r = self.op(Op::Shuffle(n, m, (index..index + l).collect()));
                self.vwrite(rd, r);
                Ok(())
            }
            Insn::SimdTable { q, tbx, regs, rd, rn, rm } => {
                let l = if q { 16 } else { 8 };
                let ty = Type::Vec(l, Elem::I8);
                let tables: Vec<Value> = (0..regs).map(|r| self.vread(rn.plus(r), Type::Vec(16, Elem::I8))).collect();
                let table = self.concat(&tables);
                let idx = self.vread(rm, ty);
                let mut out = if tbx { self.vread(rd, ty) } else { self.splat_k(ty, 0) };
                let size = self.konst(Type::I8, u128::from(regs) * 16);
                let zero = self.konst(Type::I8, 0);
                for i in 0..l {
                    let ix = self.op(Op::Extract(idx, i));
                    let inside = self.cmp(Pred::Ult, ix, size);
                    let safe = self.select(inside, ix, zero);
                    let byte = self.op(Op::ExtractAt(table, safe));
                    let old = self.op(Op::Extract(out, i));
                    let b = self.select(inside, byte, old);
                    out = self.op(Op::Insert(out, i, b));
                }
                self.vwrite(rd, out);
                Ok(())
            }
            Insn::SimdDupElem { shape, rd, rn, index } => {
                let x = self.vlane(rn, int_type(shape.size()), index);
                let r = match shape {
                    Shape::Vector(_) => self.op(Op::Splat(shape_int(shape), x)),
                    Shape::Scalar(_) => x,
                };
                self.vwrite(rd, r);
                Ok(())
            }
            Insn::SimdDupGen { arr, rd, rn } => {
                let x = self.read(rn, arr.size.0 == 3, false);
                let x = self.zext(x, int_type(arr.size));
                let r = self.op(Op::Splat(vector_of(arr, int_type(arr.size)), x));
                self.vwrite(rd, r);
                Ok(())
            }
            Insn::SimdInsElem { size, rd, dst, rn, src } => {
                let x = self.vlane(rn, int_type(size), src);
                self.vwrite_lane(rd, dst, x);
                Ok(())
            }
            Insn::SimdInsGen { size, rd, index, rn } => {
                let x = self.read(rn, size.0 == 3, false);
                let x = self.zext(x, int_type(size));
                self.vwrite_lane(rd, index, x);
                Ok(())
            }
            Insn::SimdMovToGen { signed, x64, size, rd, rn, index } => {
                let x = self.vlane(rn, int_type(size), index);
                let x = self.resize(x, if x64 { Type::I64 } else { Type::I32 }, signed);
                self.write(rd, x, false);
                Ok(())
            }
            Insn::SimdModImm { imm, rd } => {
                let ty = vector_of(imm.arr, int_type(imm.arr.size));
                let value = u128::from(imm.element());
                let r = match imm.op {
                    ModImmOp::Movi | ModImmOp::Fmov => self.splat_k(ty, value),
                    ModImmOp::Mvni => self.splat_k(ty, !value),
                    ModImmOp::Orr | ModImmOp::Bic => {
                        let d = self.vread(rd, ty);
                        if imm.op == ModImmOp::Orr {
                            let k = self.splat_k(ty, value);
                            self.bin(BinOp::Or, d, k)
                        } else {
                            let k = self.splat_k(ty, !value);
                            self.bin(BinOp::And, d, k)
                        }
                    }
                };
                self.vwrite(rd, r);
                Ok(())
            }
            _ => unreachable!("not an Advanced SIMD data-processing instruction: {insn}"),
        }
    }

    // ---- helpers

    /// ORs FPSR.QC in where `sat` (an `i1` or a lane mask) has a lane set.
    fn saturated(&mut self, sat: Value) {
        let any = if self.ty(sat).is_vector() {
            let lanes = self.ty(sat).with_elem(Type::I8);
            let wide = self.op(Op::Cast(Cast::Zext, lanes, sat));
            let top = self.op(Op::Reduce(Reduce::UMax, wide));
            self.zext(top, Type::I64)
        } else {
            self.zext(sat, Type::I64)
        };
        let bit = self.shl_k(any, QC.trailing_zeros());
        let fpsr = self.get(Loc::Fpsr);
        let new = self.bin(BinOp::Or, fpsr, bit);
        self.set(Loc::Fpsr, new);
    }

    /// A lane mask as all ones or zeros in lanes of `ty`.
    fn mask_to(&mut self, mask: Value, ty: Type) -> Value {
        self.op(Op::Cast(Cast::Sext, ty, mask))
    }

    /// A saturating `op` (SAddSat ...), with QC set where it saturated (where it differs from the wrapping one).
    fn qop(&mut self, op: BinOp, a: Value, b: Value) -> Value {
        let wrap = match op {
            BinOp::SAddSat | BinOp::UAddSat => BinOp::Add,
            _ => BinOp::Sub,
        };
        let r = self.bin(op, a, b);
        let w = self.bin(wrap, a, b);
        let differs = self.cmp(Pred::Ne, r, w);
        self.saturated(differs);
        r
    }

    /// `x` clamped to `lo ..= hi` (bit patterns of its elements; signed comparisons when `signed`), with QC set where
    /// it was outside.
    fn clamp(&mut self, x: Value, lo: u128, hi: u128, signed: bool) -> Value {
        let (min, max) = if signed { (BinOp::SMin, BinOp::SMax) } else { (BinOp::UMin, BinOp::UMax) };
        let hk = self.k_like(x, hi);
        let lk = self.k_like(x, lo);
        let r = self.bin(min, x, hk);
        let r = self.bin(max, r, lk);
        let differs = self.cmp(Pred::Ne, r, x);
        self.saturated(differs);
        r
    }

    /// A wide value saturated to half its width (SignedSatQ / UnsignedSatQ of a signed or unsigned source), narrowed.
    fn sat_narrow(&mut self, x: Value, signed_src: bool, signed_dst: bool) -> Value {
        let ty = self.ty(x);
        let e = ty.elem().bits() / 2;
        let r = match (signed_src, signed_dst) {
            (true, true) => {
                let wmask = umax(2 * e);
                self.clamp(x, (wmask - smax(e)) & wmask, smax(e), true)
            }
            (true, false) => self.clamp(x, 0, umax(e), true),
            (false, _) => self.clamp(x, 0, umax(e), false),
        };
        let narrow = ty.with_elem(Type::int(e).expect("8 to 64 bits"));
        self.op(Op::Cast(Cast::Trunc, narrow, r))
    }

    /// `x` widened to `ty` (signed or not).
    fn widen(&mut self, x: Value, ty: Type, signed: bool) -> Value {
        self.op(Op::Cast(if signed { Cast::Sext } else { Cast::Zext }, ty, x))
    }

    /// `x >> n` by a constant below the width, arithmetic or logical.
    fn shr_k(&mut self, x: Value, n: u32, signed: bool) -> Value {
        if signed { self.ashr_k(x, n) } else { self.lshr_k(x, n) }
    }

    /// Bit `n` (a constant) of each element of `x`, as 0 or 1 of its type.
    fn bit_k(&mut self, x: Value, n: u32) -> Value {
        let s = self.lshr_k(x, n);
        self.and_k(s, 1)
    }

    /// `x` shifted right by `n` (1 to the width), rounded (adding the last bit shifted out), the result for `n` the
    /// width included.
    fn rshr_k(&mut self, x: Value, n: u32, signed: bool, round: bool) -> Value {
        let w = self.ty(x).elem().bits();
        let hi = if n >= w {
            if signed { self.ashr_k(x, w - 1) } else { self.k_like(x, 0) }
        } else {
            self.shr_k(x, n, signed)
        };
        if !round {
            return hi;
        }
        let b = self.bit_k(x, n - 1);
        self.bin(BinOp::Add, hi, b)
    }

    /// The two vectors' elements paired up (the pairwise forms): their concatenation's even and odd elements.
    fn pairs(&mut self, n: Value, m: Value) -> (Value, Value) {
        let l = self.ty(n).lanes() as u8;
        let evens = self.op(Op::Shuffle(n, m, (0..l).map(|i| 2 * i).collect()));
        let odds = self.op(Op::Shuffle(n, m, (0..l).map(|i| 2 * i + 1).collect()));
        (evens, odds)
    }

    /// `|a − b|` of integers (signed or not), wrapping into their width as an unsigned value.
    fn abd(&mut self, a: Value, b: Value, signed: bool) -> Value {
        let gt = self.cmp(if signed { Pred::Sgt } else { Pred::Ugt }, a, b);
        let x = self.bin(BinOp::Sub, a, b);
        let y = self.bin(BinOp::Sub, b, a);
        self.select(gt, x, y)
    }

    /// Carry-less products of `a` and `b`'s low `bits` bits (both of one type, wide enough for the product).
    fn pmul(&mut self, a: Value, b: Value, bits: u32) -> Value {
        let mut acc = self.k_like(a, 0);
        let zero = self.k_like(a, 0);
        for i in 0..bits {
            let bit = self.bit_k(b, i);
            let set = self.cmp(Pred::Ne, bit, zero);
            let shifted = self.shl_k(a, i);
            let term = self.select(set, shifted, zero);
            acc = self.bin(BinOp::Xor, acc, term);
        }
        acc
    }

    /// The shifts by a register's element (SSHL ... UQRSHL): `x` by the signed low byte of each element of `s`, left
    /// when positive, right when negative, rounded and saturated as the form says.
    fn shift_by(&mut self, x: Value, s: Value, signed: bool, round: bool, sat: bool) -> Value {
        let ty = self.ty(x);
        let e = ty.elem().bits();
        let sh = if e == 8 {
            s
        } else {
            let bytes = self.op(Op::Cast(Cast::Trunc, ty.with_elem(Type::I8), s));
            self.widen(bytes, ty, true)
        };
        let zero = self.k_like(x, 0);
        let width = self.k_like(x, u128::from(e));
        let left = self.cmp(Pred::Sge, sh, zero);
        // (left: by sh, below the width; else every bit shifted out)
        let in_l = self.cmp(Pred::Ult, sh, width);
        let sh_l = self.select(in_l, sh, zero);
        let l = self.bin(BinOp::Shl, x, sh_l);
        let l = self.select(in_l, l, zero);
        let l = if sat {
            let back = self.bin(if signed { BinOp::AShr } else { BinOp::LShr }, l, sh_l);
            let lost_in = self.cmp(Pred::Ne, back, x);
            let nonzero = self.cmp(Pred::Ne, x, zero);
            let over = self.select(in_l, lost_in, nonzero);
            let over = self.bin(BinOp::And, over, left);
            self.saturate_left(x, l, over, signed)
        } else {
            l
        };
        // (right: by −sh, 1 to 128)
        let r_amt = self.un(UnOp::Neg, sh);
        let in_r = self.cmp(Pred::Ult, r_amt, width);
        let last = self.k_like(x, u128::from(e - 1));
        let sh_r = self.select(in_r, r_amt, last);
        let shifted = self.bin(if signed { BinOp::AShr } else { BinOp::LShr }, x, sh_r);
        let beyond = if signed { shifted } else { zero };
        let r = self.select(in_r, shifted, beyond);
        let r = if round {
            let one = self.k_like(x, 1);
            let reach = self.cmp(Pred::Ule, r_amt, width);
            let b_amt = self.bin(BinOp::Sub, r_amt, one);
            let b_amt = self.select(reach, b_amt, zero);
            let b = self.bin(BinOp::LShr, x, b_amt);
            let b = self.bin(BinOp::And, b, one);
            let b = self.select(reach, b, zero);
            let rounded = self.bin(BinOp::Add, r, b);
            let far = self.cmp(Pred::Ugt, r_amt, width);
            self.select(far, zero, rounded)
        } else {
            r
        };
        self.select(left, l, r)
    }

    /// `l`, `x` shifted left, where `over` (a lane mask) does not say it saturated; else the limit of `x`'s sign
    /// (`signed`) or the unsigned maximum, with QC set.
    fn saturate_left(&mut self, x: Value, l: Value, over: Value, signed: bool) -> Value {
        let e = self.ty(x).elem().bits();
        let limit = if signed {
            let zero = self.k_like(x, 0);
            let neg = self.cmp(Pred::Slt, x, zero);
            let (mn, mx) = (self.k_like(x, smin(e)), self.k_like(x, smax(e)));
            self.select(neg, mn, mx)
        } else {
            self.k_like(x, umax(e))
        };
        self.saturated(over);
        self.select(over, limit, l)
    }

    /// SQDMULH and SQRDMULH (`round`): the high half of twice the product, saturated (only MIN × MIN does).
    fn qdmulh(&mut self, a: Value, b: Value, round: bool) -> Value {
        let ty = self.ty(a);
        let e = ty.elem().bits();
        let wide = ty.with_elem(Type::int(2 * e).expect("16 or 32 bits"));
        let (wa, wb) = (self.widen(a, wide, true), self.widen(b, wide, true));
        let p = self.bin(BinOp::Mul, wa, wb);
        let p = if round {
            let k = self.k_like(p, 1 << (e - 2));
            self.bin(BinOp::Add, p, k)
        } else {
            p
        };
        let hi = self.ashr_k(p, e - 1);
        let r = self.op(Op::Cast(Cast::Trunc, ty, hi));
        let mn = self.k_like(a, smin(e));
        let (am, bm) = (self.cmp(Pred::Eq, a, mn), self.cmp(Pred::Eq, b, mn));
        let both = self.bin(BinOp::And, am, bm);
        self.saturated(both);
        let mx = self.k_like(a, smax(e));
        self.select(both, mx, r)
    }

    /// SQRDMLAH and SQRDMLSH (`sub`): `d` plus (or minus) the rounded high half of twice the product, saturated.
    fn qrdmlah(&mut self, d: Value, a: Value, b: Value, sub: bool) -> Value {
        let ty = self.ty(a);
        let e = ty.elem().bits();
        let wide = ty.with_elem(Type::int(2 * e).expect("16 or 32 bits"));
        let (wa, wb) = (self.widen(a, wide, true), self.widen(b, wide, true));
        let p = self.bin(BinOp::Mul, wa, wb);
        let p = if sub { self.un(UnOp::Neg, p) } else { p };
        let k = self.k_like(p, 1 << (e - 2));
        let p = self.bin(BinOp::Add, p, k);
        let t = self.ashr_k(p, e - 1);
        let wd = self.widen(d, wide, true);
        let sum = self.bin(BinOp::Add, wd, t);
        self.sat_narrow(sum, true, true)
    }

    /// SQDMULL: twice the product of the widened `a` and `b`, saturated.
    fn qdmull(&mut self, a: Value, b: Value) -> Value {
        let p = self.bin(BinOp::Mul, a, b);
        self.qop(BinOp::SAddSat, p, p)
    }

    // ---- three same

    fn simd3(&mut self, op: Simd3Op, shape: Shape, rd: VReg, rn: VReg, rm: VReg) -> Result<(), Fault> {
        use Simd3Op::*;
        let fp = op.is_fp();
        if fp {
            float_elem(shape.size().bytes() * 8, !matches!(op, Fmla | Fmls | Frecps | Frsqrts))?;
            return self.simd3_fp(op, shape, rd, rn, rm);
        }
        if matches!(op, Sdot | Udot) {
            let m = self.vread(rm, dot_bytes(shape));
            self.dot(shape, rd, rn, m, op == Sdot);
            return Ok(());
        }
        let ty = shape_int(shape);
        let (n, m) = (self.vread(rn, ty), self.vread(rm, ty));
        let e = ty.elem().bits();
        let r = match op {
            Add => self.bin(BinOp::Add, n, m),
            Sub => self.bin(BinOp::Sub, n, m),
            Mul => self.bin(BinOp::Mul, n, m),
            And => self.bin(BinOp::And, n, m),
            Orr => self.bin(BinOp::Or, n, m),
            Eor => self.bin(BinOp::Xor, n, m),
            Bic | Orn => {
                let nm = self.un(UnOp::Not, m);
                self.bin(if op == Bic { BinOp::And } else { BinOp::Or }, n, nm)
            }
            Bsl | Bit | Bif => {
                let d = self.vread(rd, ty);
                // (BSL: d selects n or m; BIT: m selects n or d; BIF: m selects d or n)
                let (sel, ones, zeros) = match op {
                    Bsl => (d, n, m),
                    Bit => (m, n, d),
                    _ => (m, d, n),
                };
                let a = self.bin(BinOp::And, sel, ones);
                let ns = self.un(UnOp::Not, sel);
                let b = self.bin(BinOp::And, ns, zeros);
                self.bin(BinOp::Or, a, b)
            }
            Mla | Mls => {
                let d = self.vread(rd, ty);
                let p = self.bin(BinOp::Mul, n, m);
                self.bin(if op == Mla { BinOp::Add } else { BinOp::Sub }, d, p)
            }
            Pmul => self.pmul(n, m, 8),
            Cmgt | Cmge | Cmhi | Cmhs | Cmeq | Cmtst => {
                let mask = match op {
                    Cmgt => self.cmp(Pred::Sgt, n, m),
                    Cmge => self.cmp(Pred::Sge, n, m),
                    Cmhi => self.cmp(Pred::Ugt, n, m),
                    Cmhs => self.cmp(Pred::Uge, n, m),
                    Cmeq => self.cmp(Pred::Eq, n, m),
                    _ => {
                        let a = self.bin(BinOp::And, n, m);
                        let z = self.k_like(a, 0);
                        self.cmp(Pred::Ne, a, z)
                    }
                };
                self.mask_to(mask, ty)
            }
            Smax => self.bin(BinOp::SMax, n, m),
            Umax => self.bin(BinOp::UMax, n, m),
            Smin => self.bin(BinOp::SMin, n, m),
            Umin => self.bin(BinOp::UMin, n, m),
            Sqadd => self.qop(BinOp::SAddSat, n, m),
            Uqadd => self.qop(BinOp::UAddSat, n, m),
            Sqsub => self.qop(BinOp::SSubSat, n, m),
            Uqsub => self.qop(BinOp::USubSat, n, m),
            Shadd | Uhadd | Srhadd | Urhadd => {
                // ((n + m) >> 1, rounded: n >> 1 + m >> 1 + the low bits' carry)
                let signed = matches!(op, Shadd | Srhadd);
                let (a, b) = (self.shr_k(n, 1, signed), self.shr_k(m, 1, signed));
                let low = self.bin(if matches!(op, Srhadd | Urhadd) { BinOp::Or } else { BinOp::And }, n, m);
                let low = self.and_k(low, 1);
                let s = self.bin(BinOp::Add, a, b);
                self.bin(BinOp::Add, s, low)
            }
            Shsub | Uhsub => {
                let signed = op == Shsub;
                let (a, b) = (self.shr_k(n, 1, signed), self.shr_k(m, 1, signed));
                let nn = self.un(UnOp::Not, n);
                let borrow = self.bin(BinOp::And, nn, m);
                let borrow = self.and_k(borrow, 1);
                let s = self.bin(BinOp::Sub, a, b);
                self.bin(BinOp::Sub, s, borrow)
            }
            Sabd | Uabd => self.abd(n, m, op == Sabd),
            Saba | Uaba => {
                let d = self.vread(rd, ty);
                let a = self.abd(n, m, op == Saba);
                self.bin(BinOp::Add, d, a)
            }
            Sshl | Ushl | Srshl | Urshl | Sqshl | Uqshl | Sqrshl | Uqrshl => {
                let signed = matches!(op, Sshl | Srshl | Sqshl | Sqrshl);
                let round = matches!(op, Srshl | Urshl | Sqrshl | Uqrshl);
                let sat = matches!(op, Sqshl | Uqshl | Sqrshl | Uqrshl);
                self.shift_by(n, m, signed, round, sat)
            }
            Smaxp | Umaxp | Sminp | Uminp | Addp => {
                let (a, b) = self.pairs(n, m);
                let o = match op {
                    Smaxp => BinOp::SMax,
                    Umaxp => BinOp::UMax,
                    Sminp => BinOp::SMin,
                    Uminp => BinOp::UMin,
                    _ => BinOp::Add,
                };
                self.bin(o, a, b)
            }
            Sqdmulh => self.qdmulh(n, m, false),
            Sqrdmulh => self.qdmulh(n, m, true),
            Sqrdmlah | Sqrdmlsh => {
                let d = self.vread(rd, ty);
                self.qrdmlah(d, n, m, op == Sqrdmlsh)
            }
            _ => unreachable!("an FP or dot product form: {op:?} {e}"),
        };
        self.vwrite(rd, r);
        Ok(())
    }

    /// SDOT and UDOT: each word of `rd` (of `shape`, 2S or 4S) plus the four products of its bytes of `rn` and of `m`
    /// (the bytes of `dot_bytes(shape)`), written back.
    fn dot(&mut self, shape: Shape, rd: VReg, rn: VReg, m: Value, signed: bool) {
        let n = self.vread(rn, dot_bytes(shape));
        let d = self.vread(rd, shape_int(shape));
        let bytes = self.ty(n).lanes() as u8;
        let wide = Type::Vec(bytes, Elem::I32);
        let (a, b) = (self.widen(n, wide, signed), self.widen(m, wide, signed));
        let p = self.bin(BinOp::Mul, a, b);
        let mut acc = d;
        for j in 0..4u8 {
            let part = self.op(Op::Shuffle(p, p, (0..bytes / 4).map(|k| 4 * k + j).collect()));
            acc = self.bin(BinOp::Add, acc, part);
        }
        self.vwrite(rd, acc);
    }

    /// SMULL ... SQDMLSL (three different, or by element): `n` × `m` (widened to `wide`), alone or added to or taken
    /// from `rd`, saturating for the doubling forms.
    fn widening_mul(&mut self, kind: WideMul, rd: VReg, wide: Type, n: Value, m: Value) -> Value {
        let doubled = matches!(kind, WideMul::QMul | WideMul::QAcc | WideMul::QSub);
        let p = if doubled { self.qdmull(n, m) } else { self.bin(BinOp::Mul, n, m) };
        let acc = match kind {
            WideMul::Mul | WideMul::QMul => return p,
            WideMul::Acc => BinOp::Add,
            WideMul::Sub => BinOp::Sub,
            WideMul::QAcc => BinOp::SAddSat,
            WideMul::QSub => BinOp::SSubSat,
        };
        let d = self.vread(rd, wide);
        if doubled { self.qop(acc, d, p) } else { self.bin(acc, d, p) }
    }

    fn simd3_fp(&mut self, op: Simd3Op, shape: Shape, rd: VReg, rn: VReg, rm: VReg) -> Result<(), Fault> {
        use Simd3Op::*;
        let ty = shape_float(shape);
        let env = self.fp_env();
        let (n, m) = (self.vread(rn, ty), self.vread(rm, ty));
        let r = match op {
            Fadd => self.fp(Fast::Arith(FBinOp::Add), &[n, m]),
            Fsub => self.fp(Fast::Arith(FBinOp::Sub), &[n, m]),
            Fmul => self.fp(Fast::Arith(FBinOp::Mul), &[n, m]),
            Fdiv => self.fp(Fast::Arith(FBinOp::Div), &[n, m]),
            Fmulx => self.fp(Fast::MulX, &[n, m]),
            Fabd => {
                let d = self.fp(Fast::Arith(FBinOp::Sub), &[n, m]);
                self.fabs(d)
            }
            Fmax => self.fp(Fast::MinMax { max: true, nm: false }, &[n, m]),
            Fmin => self.fp(Fast::MinMax { max: false, nm: false }, &[n, m]),
            Fmaxnm => self.fp(Fast::MinMax { max: true, nm: true }, &[n, m]),
            Fminnm => self.fp(Fast::MinMax { max: false, nm: true }, &[n, m]),
            Fmla | Fmls => {
                let d = self.vread(rd, ty);
                let n = if op == Fmls { self.fneg(n) } else { n };
                self.fp(Fast::MulAdd, &[d, n, m])
            }
            Fcmeq | Fcmge | Fcmgt | Facge | Facgt => {
                let (a, b) = if matches!(op, Facge | Facgt) { (self.fabs(n), self.fabs(m)) } else { (n, m) };
                let pred = match op {
                    Fcmeq => FPred::Oeq,
                    Fcmge | Facge => FPred::Oge,
                    _ => FPred::Ogt,
                };
                let mask = self.fcompare(env, pred, a, b);
                self.mask_to(mask, ty.as_int())
            }
            Frecps => self.fp(Fast::Step { rsqrt: false }, &[n, m]),
            Frsqrts => self.fp(Fast::Step { rsqrt: true }, &[n, m]),
            Faddp | Fmaxp | Fminp | Fmaxnmp | Fminnmp => {
                let (a, b) = self.pairs(n, m);
                match op {
                    Faddp => self.fp(Fast::Arith(FBinOp::Add), &[a, b]),
                    Fmaxp => self.fp(Fast::MinMax { max: true, nm: false }, &[a, b]),
                    Fminp => self.fp(Fast::MinMax { max: false, nm: false }, &[a, b]),
                    Fmaxnmp => self.fp(Fast::MinMax { max: true, nm: true }, &[a, b]),
                    _ => self.fp(Fast::MinMax { max: false, nm: true }, &[a, b]),
                }
            }
            _ => unreachable!("not an FP three-same form: {op:?}"),
        };
        self.vwrite(rd, r);
        Ok(())
    }

    /// FRECPS (2 − n × m) and FRSQRTS ((3 − n × m) ÷ 2), fused: infinity times zero gives 2.0 and 1.5.
    pub(crate) fn step(&mut self, env: crate::fp::Env, n: Value, m: Value, rsqrt: bool) -> Value {
        let neg = self.fneg(n);
        let a = self.flush(env, neg);
        let b = self.flush(env, m);
        let f = self.fmt(a);
        let r = self.step_core(a, b, rsqrt);
        let r = self.nan_fix(env, r, &[a, b]);
        let special = self.inf_times_zero(a, b);
        let k = if rsqrt { f.pow2(0) | (1 << (f.frac - 1)) } else { f.pow2(1) };
        let k = self.k_like(a, k);
        let r = self.select(special, k, r);
        self.flush(env, r)
    }

    /// The host's 2 + a × b (FRECPS) or (3 + a × b) ÷ 2 (FRSQRTS), `a` the negated operand: [`Sem::step`] without its
    /// NaNs, its flushes and its infinity times zero.
    pub(crate) fn step_core(&mut self, a: Value, b: Value, rsqrt: bool) -> Value {
        let f = self.fmt(a);
        if rsqrt {
            // ((3 − n × m) ÷ 2, rounded once: with m ÷ 2 where that is exact, else halved after)
            let half = self.k_like(b, f.pow2(-1));
            let hb = self.op(Op::FBin(FBinOp::Mul, b, half));
            let k15 = self.k_like(a, f.pow2(0) | (1 << (f.frac - 1)));
            let fine = self.op(Op::Fma(a, hb, k15));
            let three = self.k_like(a, f.pow2(1) | (1 << (f.frac - 1)));
            let coarse = self.op(Op::Fma(a, b, three));
            let coarse = self.op(Op::FBin(FBinOp::Mul, coarse, half));
            let ab = self.fabs(b);
            let small = self.k_like(b, f.pow2(2 - f.bias() as i32));
            let exact = self.op(Op::FCmp(FPred::Oge, ab, small));
            self.select(exact, fine, coarse)
        } else {
            let two = self.k_like(a, f.pow2(1));
            self.op(Op::Fma(a, b, two))
        }
    }

    // ---- three different

    fn simd_diff(&mut self, op: DiffOp, shape: Shape, rd: VReg, rn: VReg, rm: VReg) -> Result<(), Fault> {
        use DiffOp::*;
        let up = upper(shape);
        if op == Pmull && shape.size().0 == 3 {
            let a = self.vread_part(rn, Type::I64, up);
            let b = self.vread_part(rm, Type::I64, up);
            let (a, b) = (self.zext(a, Type::I128), self.zext(b, Type::I128));
            let r = self.pmul(a, b, 64);
            self.vwrite(rd, r);
            return Ok(());
        }
        let (nt, wt) = (narrow_type(shape), wide_type(shape));
        let signed = matches!(
            op,
            Saddl | Saddw | Ssubl | Ssubw | Sabal | Sabdl | Smlal | Sqdmlal | Smlsl | Sqdmlsl | Smull | Sqdmull
        );
        if op.form() == DiffForm::Narrow {
            let (n, m) = (self.vread(rn, wt), self.vread(rm, wt));
            let e = nt.elem().bits();
            let s = self.bin(if matches!(op, Addhn | Raddhn) { BinOp::Add } else { BinOp::Sub }, n, m);
            let s = if matches!(op, Raddhn | Rsubhn) {
                let k = self.k_like(s, 1 << (e - 1));
                self.bin(BinOp::Add, s, k)
            } else {
                s
            };
            let hi = self.lshr_k(s, e);
            let r = self.op(Op::Cast(Cast::Trunc, nt, hi));
            self.vwrite_part(rd, r, up);
            return Ok(());
        }
        let n = if op.form() == DiffForm::Wide {
            self.vread(rn, wt)
        } else {
            let x = self.vread_part(rn, nt, up);
            self.widen(x, wt, signed)
        };
        let m = self.vread_part(rm, nt, up);
        let m = if op == Pmull { self.widen(m, wt, false) } else { self.widen(m, wt, signed) };
        let r = match op {
            Saddl | Uaddl | Saddw | Uaddw => self.bin(BinOp::Add, n, m),
            Ssubl | Usubl | Ssubw | Usubw => self.bin(BinOp::Sub, n, m),
            Sabdl | Uabdl => self.abd(n, m, signed),
            Sabal | Uabal => {
                let d = self.vread(rd, wt);
                let a = self.abd(n, m, signed);
                self.bin(BinOp::Add, d, a)
            }
            Smull | Umull => self.widening_mul(WideMul::Mul, rd, wt, n, m),
            Smlal | Umlal => self.widening_mul(WideMul::Acc, rd, wt, n, m),
            Smlsl | Umlsl => self.widening_mul(WideMul::Sub, rd, wt, n, m),
            Sqdmull => self.widening_mul(WideMul::QMul, rd, wt, n, m),
            Sqdmlal => self.widening_mul(WideMul::QAcc, rd, wt, n, m),
            Sqdmlsl => self.widening_mul(WideMul::QSub, rd, wt, n, m),
            Pmull => {
                let n = self.vread_part(rn, nt, up);
                let n = self.widen(n, wt, false);
                self.pmul(n, m, 8)
            }
            _ => unreachable!("a narrowing form: {op:?}"),
        };
        self.vwrite(rd, r);
        Ok(())
    }

    // ---- two registers

    fn simd2(&mut self, op: Simd2Op, shape: Shape, rd: VReg, rn: VReg) -> Result<(), Fault> {
        use Simd2Op::*;
        let ty = shape_int(shape);
        let e = ty.elem().bits();
        let up = upper(shape);
        let r = match op {
            Rev64 | Rev32 | Rev16 => {
                let container = match op {
                    Rev64 => 64,
                    Rev32 => 32,
                    _ => 16,
                } / e;
                let x = self.vread(rn, ty);
                let l = ty.lanes();
                let mask =
                    (0..l).map(|i| ((i / container) * container + container - 1 - i % container) as u8).collect();
                self.op(Op::Shuffle(x, x, mask))
            }
            Saddlp | Uaddlp | Sadalp | Uadalp => {
                let x = self.vread(rn, ty);
                let signed = matches!(op, Saddlp | Sadalp);
                let l = ty.lanes() as u8;
                let wide_e = Type::int(2 * e).expect("16 to 64 bits");
                let (evens, odds, wide) = if l == 2 {
                    (self.op(Op::Extract(x, 0)), self.op(Op::Extract(x, 1)), wide_e)
                } else {
                    let half = Type::Vec(l / 2, Elem::of(ty.elem()).expect("an element"));
                    let evens = self.op(Op::Shuffle(x, x, (0..l / 2).map(|i| 2 * i).collect()));
                    let odds = self.op(Op::Shuffle(x, x, (0..l / 2).map(|i| 2 * i + 1).collect()));
                    (evens, odds, half.with_elem(wide_e))
                };
                let (a, b) = (self.widen(evens, wide, signed), self.widen(odds, wide, signed));
                let s = self.bin(BinOp::Add, a, b);
                if matches!(op, Sadalp | Uadalp) {
                    let d = self.vread(rd, wide);
                    self.bin(BinOp::Add, d, s)
                } else {
                    s
                }
            }
            Suqadd | Usqadd => {
                let x = self.vread(rn, ty);
                let d = self.vread(rd, ty);
                if op == Suqadd {
                    // (signed d + unsigned x: above MAX saturates)
                    let mx = self.k_like(d, smax(e));
                    let room = self.bin(BinOp::Sub, mx, d);
                    let over = self.cmp(Pred::Ugt, x, room);
                    self.saturated(over);
                    let s = self.bin(BinOp::Add, d, x);
                    self.select(over, mx, s)
                } else {
                    // (unsigned d + signed x: above UMAX or below 0 saturates)
                    let zero = self.k_like(x, 0);
                    let neg = self.cmp(Pred::Slt, x, zero);
                    let room = self.un(UnOp::Not, d);
                    let over = self.cmp(Pred::Ugt, x, room);
                    let nx = self.un(UnOp::Neg, x);
                    let under = self.cmp(Pred::Ugt, nx, d);
                    let bad = self.select(neg, under, over);
                    self.saturated(bad);
                    let s = self.bin(BinOp::Add, d, x);
                    let mx = self.k_like(x, umax(e));
                    let limit = self.select(neg, zero, mx);
                    self.select(bad, limit, s)
                }
            }
            Cls => {
                let x = self.vread(rn, ty);
                let s = self.ashr_k(x, 1);
                let t = self.bin(BinOp::Xor, x, s);
                let z = self.un(UnOp::Ctlz, t);
                let one = self.k_like(z, 1);
                self.bin(BinOp::Sub, z, one)
            }
            Clz => {
                let x = self.vread(rn, ty);
                self.un(UnOp::Ctlz, x)
            }
            Cnt => {
                let x = self.vread(rn, ty);
                self.un(UnOp::Ctpop, x)
            }
            Not => {
                let x = self.vread(rn, ty);
                self.un(UnOp::Not, x)
            }
            Rbit => {
                let x = self.vread(rn, ty);
                self.un(UnOp::BitReverse, x)
            }
            Abs | Neg | Sqabs | Sqneg => {
                let x = self.vread(rn, ty);
                let zero = self.k_like(x, 0);
                let neg = self.un(UnOp::Neg, x);
                let r = if matches!(op, Abs | Sqabs) {
                    let lt = self.cmp(Pred::Slt, x, zero);
                    self.select(lt, neg, x)
                } else {
                    neg
                };
                if matches!(op, Sqabs | Sqneg) {
                    let mn = self.k_like(x, smin(e));
                    let is_min = self.cmp(Pred::Eq, x, mn);
                    self.saturated(is_min);
                    let mx = self.k_like(x, smax(e));
                    self.select(is_min, mx, r)
                } else {
                    r
                }
            }
            Cmp0(c) => {
                let x = self.vread(rn, ty);
                let zero = self.k_like(x, 0);
                let pred = match c {
                    CmpZero::Gt => Pred::Sgt,
                    CmpZero::Ge => Pred::Sge,
                    CmpZero::Eq => Pred::Eq,
                    CmpZero::Le => Pred::Sle,
                    CmpZero::Lt => Pred::Slt,
                };
                let m = self.cmp(pred, x, zero);
                self.mask_to(m, ty)
            }
            Xtn | Sqxtn | Uqxtn | Sqxtun => {
                let (nt, wt) = (narrow_type(shape), wide_type(shape));
                let x = self.vread(rn, wt);
                let r = match op {
                    Xtn => self.op(Op::Cast(Cast::Trunc, nt, x)),
                    Sqxtn => self.sat_narrow(x, true, true),
                    Uqxtn => self.sat_narrow(x, false, false),
                    _ => self.sat_narrow(x, true, false),
                };
                self.vwrite_part(rd, r, up && !shape.is_scalar());
                return Ok(());
            }
            Shll => {
                let (nt, wt) = (narrow_type(shape), wide_type(shape));
                let x = self.vread_part(rn, nt, up);
                let w = self.widen(x, wt, false);
                self.shl_k(w, e)
            }
            Fcvtn | Fcvtxn => {
                let env = self.fp_env();
                let narrow = narrow_type(shape).with_elem(Type::float(e).expect("an FP format"));
                let wide = narrow.with_elem(Type::float(2 * e).expect("an FP format"));
                let x = self.vread(rn, wide);
                let r = if op == Fcvtn { self.fp(Fast::Convert(narrow), &[x]) } else { self.round_odd(env, x, narrow) };
                self.vwrite_part(rd, r, up && !shape.is_scalar());
                return Ok(());
            }
            Fcvtl => {
                let narrow = narrow_type(shape).with_elem(Type::float(e).expect("an FP format"));
                let wide = narrow.with_elem(Type::float(2 * e).expect("an FP format"));
                let x = self.vread_part(rn, narrow, up);
                self.fp(Fast::Convert(wide), &[x])
            }
            Urecpe | Ursqrte => {
                let x = self.vread(rn, ty);
                self.unsigned_estimate(x, op == Ursqrte)
            }
            Fabs | Fneg => {
                let x = self.vread(rn, shape_float(shape));
                if op == Fabs { self.fabs(x) } else { self.fneg(x) }
            }
            Frint(_) | FcvtInt { .. } | Scvtf | Ucvtf | Frecpe | Frsqrte | Frecpx | Fcmp0(_) | Fsqrt => {
                float_elem(e, !matches!(op, Frecpe | Frsqrte | Frecpx))?;
                let env = self.fp_env();
                let fty = shape_float(shape);
                match op {
                    Scvtf | Ucvtf => {
                        let x = self.vread(rn, ty);
                        self.inttof(x, 0, fty, op == Ucvtf)
                    }
                    _ => {
                        let x = self.vread(rn, fty);
                        match op {
                            Frint(round) => {
                                let op = rounding(round).unwrap_or(FUnOp::Trunc);
                                self.fp(Fast::Rint(op), &[x])
                            }
                            FcvtInt { round, unsigned } => {
                                self.fp(Fast::ToInt { fbits: 0, round: rounding(round), to: ty, unsigned }, &[x])
                            }
                            Frecpe => self.recip_estimate(env, x),
                            Frsqrte => self.rsqrt_estimate(env, x),
                            Frecpx => self.recip_exponent(env, x),
                            Fsqrt => self.fp(Fast::Sqrt, &[x]),
                            Fcmp0(c) => {
                                let zero = self.k_like(x, 0);
                                let pred = match c {
                                    CmpZero::Gt => FPred::Ogt,
                                    CmpZero::Ge => FPred::Oge,
                                    CmpZero::Eq => FPred::Oeq,
                                    CmpZero::Le => FPred::Ole,
                                    CmpZero::Lt => FPred::Olt,
                                };
                                let m = self.fcompare(env, pred, x, zero);
                                self.mask_to(m, ty)
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        };
        self.vwrite(rd, r);
        Ok(())
    }

    /// FCVTXN's conversion: rounded to odd (towards zero, then the last bit set if anything was lost).
    fn round_odd(&mut self, env: crate::fp::Env, x: Value, to: Type) -> Value {
        let x = self.flush(env, x);
        let r = self.op(Op::FCast(mir::FCast::Trunc, to, x));
        let back = self.op(Op::FCast(mir::FCast::Ext, self.ty(x), r));
        // (inexact: the narrow value back is not the operand, which is no NaN)
        let same = self.op(Op::FCmp(FPred::Oeq, back, x));
        let differs = self.un(UnOp::Not, same);
        let nan = self.is_nan(x);
        let number = self.un(UnOp::Not, nan);
        let differs = self.bin(BinOp::And, differs, number);
        let (ab, ax) = (self.fabs(back), self.fabs(x));
        let above = self.op(Op::FCmp(FPred::Ogt, ab, ax));
        let bits = self.as_int(r);
        let one = self.k_like(bits, 1);
        let down = self.bin(BinOp::Sub, bits, one);
        let truncated = self.select(above, down, bits);
        let odd = self.bin(BinOp::Or, truncated, one);
        let out = self.select(differs, odd, bits);
        let out = self.floats_of(to, out);
        let out = self.flush(env, out);
        let nan = self.convert_nan(env, x, to);
        let is = self.is_nan(x);
        self.select(is, nan, out)
    }

    /// URECPE and URSQRTE (`sqrt`): the estimates of words as fixed-point fractions.
    fn unsigned_estimate(&mut self, x: Value, sqrt: bool) -> Value {
        let ones = self.k_like(x, u128::from(u32::MAX));
        let top = self.lshr_k(x, 23);
        let est = if sqrt { self.rsqrt_table(top) } else { self.recip_table(top) };
        let r = self.shl_k(est, 23);
        let small = if sqrt {
            let t = self.lshr_k(x, 30);
            let z = self.k_like(t, 0);
            self.cmp(Pred::Eq, t, z)
        } else {
            let t = self.lshr_k(x, 31);
            let z = self.k_like(t, 0);
            self.cmp(Pred::Eq, t, z)
        };
        self.select(small, ones, r)
    }

    /// RecipEstimate of each element (256 to 511): 256 to 511.
    fn recip_table(&mut self, a: Value) -> Value {
        let two = self.k_like(a, 2);
        let one = self.k_like(a, 1);
        let a = self.bin(BinOp::Mul, a, two);
        let a = self.bin(BinOp::Add, a, one);
        let k = self.k_like(a, 1 << 19);
        let b = self.bin(BinOp::UDiv, k, a);
        let b = self.bin(BinOp::Add, b, one);
        self.lshr_k(b, 1)
    }

    /// RecipSqrtEstimate of each element (128 to 511): 256 to 511, from its table.
    fn rsqrt_table(&mut self, a: Value) -> Value {
        let ty = self.ty(a);
        let base = self.k_like(a, 128);
        let idx = self.bin(BinOp::Sub, a, base);
        // (an operand below 128 is a lane whose result is chosen elsewhere: any entry will do)
        let last = self.k_like(a, 383);
        let idx = self.bin(BinOp::UMin, idx, last);
        let mut out = self.k_like(a, 0);
        for lane in 0..ty.lanes() as u8 {
            let i = if ty.is_vector() { self.op(Op::Extract(idx, lane)) } else { idx };
            let b = self.op(Op::Lookup(Table::RecipSqrt, i));
            let b = self.zext(b, ty.elem());
            let k = self.konst(ty.elem(), 256);
            let v = self.bin(BinOp::Add, b, k);
            out = if ty.is_vector() { self.op(Op::Insert(out, lane, v)) } else { v };
        }
        out
    }

    /// FPRecipEstimate (FRECPE).
    fn recip_estimate(&mut self, env: crate::fp::Env, x: Value) -> Value {
        let ty = self.ty(x);
        let x = self.flush(env, x);
        let f = self.fmt(x);
        let fr = f.frac;
        let bits = self.as_int(x);
        let sign = self.and_k(bits, f.sign());
        let abs = self.and_k(bits, !f.sign() & umax(f.bits));
        let exp = self.lshr_k(abs, fr);
        let frac = self.and_k(bits, f.frac_mask());
        let zero = self.k_like(bits, 0);
        // (a subnormal: normalized by one or two places, its exponent 0 or −1)
        let den = self.cmp(Pred::Eq, exp, zero);
        let top = self.bit_k(frac, fr - 1);
        let top0 = self.cmp(Pred::Eq, top, zero);
        let f2 = self.shl_k(frac, 2);
        let f2 = self.and_k(f2, f.frac_mask());
        let f1 = self.shl_k(frac, 1);
        let f1 = self.and_k(f1, f.frac_mask());
        let fd = self.select(top0, f2, f1);
        let frac = self.select(den, fd, frac);
        let minus1 = self.k_like(bits, umax(f.bits));
        let two_down = self.bin(BinOp::And, den, top0);
        let exp = self.select(two_down, minus1, exp);
        let hi = self.lshr_k(frac, fr - 8);
        let k256 = self.k_like(hi, 256);
        let scaled = self.bin(BinOp::Or, hi, k256);
        let est = self.recip_table(scaled);
        let base = self.k_like(exp, 2 * f.bias() - 1);
        let rexp = self.bin(BinOp::Sub, base, exp);
        let low = self.and_k(est, 0xff);
        let rfrac = self.shl_k(low, fr - 8);
        // (a result exponent of 0 or −1: the fraction shifted down into a subnormal)
        let q1 = self.k_like(rfrac, 1 << (fr - 1));
        let s1 = self.lshr_k(rfrac, 1);
        let sub1 = self.bin(BinOp::Or, q1, s1);
        let q2 = self.k_like(rfrac, 1 << (fr - 2));
        let s2 = self.lshr_k(rfrac, 2);
        let sub2 = self.bin(BinOp::Or, q2, s2);
        let is0 = self.cmp(Pred::Eq, rexp, zero);
        let is_m1 = self.cmp(Pred::Eq, rexp, minus1);
        let rfrac = self.select(is0, sub1, rfrac);
        let rfrac = self.select(is_m1, sub2, rfrac);
        let rexp = self.select(is_m1, zero, rexp);
        let rexp = self.and_k(rexp, (1 << f.exp_bits()) - 1);
        let rexp = self.shl_k(rexp, fr);
        let r = self.bin(BinOp::Or, sign, rexp);
        let r = self.bin(BinOp::Or, r, rfrac);
        // (the special cases: an infinity gives a zero, a zero or a value too small an infinity, and under FZ a value
        // too large a zero)
        let inf = self.k_like(bits, f.inf());
        let signed_inf = self.bin(BinOp::Or, sign, inf);
        let tiny_k = self.k_like(abs, 1 << (fr - 2));
        let tiny = self.cmp(Pred::Ult, abs, tiny_k);
        let r = self.select(tiny, signed_inf, r);
        let big_k = self.k_like(abs, f.pow2(f.bias() as i32 - 1));
        let big = self.cmp(Pred::Uge, abs, big_k);
        let never = self.k_like(big, 0);
        let big = self.select(env.fz, big, never);
        let r = self.select(big, sign, r);
        let is_inf = self.cmp(Pred::Eq, abs, inf);
        let r = self.select(is_inf, sign, r);
        let r = self.floats_of(ty, r);
        let nan = self.is_nan(x);
        let fixed = self.nan_fix(env, x, &[x]);
        self.select(nan, fixed, r)
    }

    /// FPRSqrtEstimate (FRSQRTE).
    fn rsqrt_estimate(&mut self, env: crate::fp::Env, x: Value) -> Value {
        let ty = self.ty(x);
        let x = self.flush(env, x);
        let f = self.fmt(x);
        let fr = f.frac;
        let w = f.bits;
        let bits = self.as_int(x);
        let sign = self.and_k(bits, f.sign());
        let abs = self.and_k(bits, !f.sign() & umax(w));
        let exp = self.lshr_k(abs, fr);
        let frac = self.and_k(bits, f.frac_mask());
        let zero = self.k_like(bits, 0);
        // (a subnormal: normalized, its exponent 0 less the places)
        let den = self.cmp(Pred::Eq, exp, zero);
        let lz = self.un(UnOp::Ctlz, frac);
        let lead = self.k_like(lz, u128::from(w - fr));
        let lz = self.bin(BinOp::Sub, lz, lead);
        let one = self.k_like(lz, 1);
        let by = self.bin(BinOp::Add, lz, one);
        let fmax = self.k_like(by, u128::from(w - 1));
        let by = self.bin(BinOp::UMin, by, fmax);
        let fd = self.bin(BinOp::Shl, frac, by);
        let fd = self.and_k(fd, f.frac_mask());
        let ed = self.un(UnOp::Neg, lz);
        let frac = self.select(den, fd, frac);
        let exp = self.select(den, ed, exp);
        let odd = self.bit_k(exp, 0);
        let even = self.cmp(Pred::Eq, odd, zero);
        let h7 = self.lshr_k(frac, fr - 7);
        let k128 = self.k_like(h7, 128);
        let top7 = self.bin(BinOp::Or, h7, k128);
        let h8 = self.lshr_k(frac, fr - 8);
        let k256 = self.k_like(h8, 256);
        let top8 = self.bin(BinOp::Or, h8, k256);
        // (an even exponent: 1:fraction's top 8 bits, 256 to 511; an odd one 01:the top 7, 128 to 255)
        let scaled = self.select(even, top8, top7);
        let est = self.rsqrt_table(scaled);
        let base = self.k_like(exp, 3 * f.bias() - 1);
        let rexp = self.bin(BinOp::Sub, base, exp);
        let rexp = self.lshr_k(rexp, 1);
        let rexp = self.and_k(rexp, (1 << f.exp_bits()) - 1);
        let rexp = self.shl_k(rexp, fr);
        let low = self.and_k(est, 0xff);
        let rfrac = self.shl_k(low, fr - 8);
        let r = self.bin(BinOp::Or, rexp, rfrac);
        // (a zero gives an infinity of its sign; a negative number the default NaN; +infinity +0)
        let inf = self.k_like(bits, f.inf());
        let signed_inf = self.bin(BinOp::Or, sign, inf);
        let is_zero = self.cmp(Pred::Eq, abs, zero);
        let negative = self.cmp(Pred::Ne, sign, zero);
        let dnan = self.k_like(bits, f.default_nan());
        let is_inf = self.cmp(Pred::Eq, abs, inf);
        let r = self.select(is_inf, zero, r);
        let r = self.select(negative, dnan, r);
        let r = self.select(is_zero, signed_inf, r);
        let r = self.floats_of(ty, r);
        let nan = self.is_nan(x);
        let fixed = self.nan_fix(env, x, &[x]);
        self.select(nan, fixed, r)
    }

    /// FPRecpX (FRECPX): the exponent inverted, the fraction zero.
    fn recip_exponent(&mut self, env: crate::fp::Env, x: Value) -> Value {
        let ty = self.ty(x);
        let x = self.flush(env, x);
        let f = self.fmt(x);
        let bits = self.as_int(x);
        let sign = self.and_k(bits, f.sign());
        let exp = self.and_k(bits, f.exp());
        let zero = self.k_like(bits, 0);
        let is0 = self.cmp(Pred::Eq, exp, zero);
        let inv = self.un(UnOp::Not, exp);
        let inv = self.and_k(inv, f.exp());
        let max = self.k_like(bits, f.exp() - (1 << f.frac));
        let e = self.select(is0, max, inv);
        let r = self.bin(BinOp::Or, sign, e);
        let r = self.floats_of(ty, r);
        let nan = self.is_nan(x);
        let fixed = self.nan_fix(env, x, &[x]);
        self.select(nan, fixed, r)
    }

    // ---- across lanes

    fn across(&mut self, op: AcrossOp, arr: Arr, rd: VReg, rn: VReg) -> Result<(), Fault> {
        use AcrossOp::*;
        if matches!(op, Fmaxnmv | Fminnmv | Fmaxv | Fminv) {
            let ty = vector_of(arr, float_elem(arr.size.bytes() * 8, true)?);
            let x = self.vread(rn, ty);
            let max = matches!(op, Fmaxnmv | Fmaxv);
            let nm = matches!(op, Fmaxnmv | Fminnmv);
            // (the Arm ARM's Reduce: the lower half's result with the upper half's, recursively, so pairs of
            // neighbours first)
            let mut e: Vec<Value> = (0..ty.lanes() as u8).map(|i| self.op(Op::Extract(x, i))).collect();
            while e.len() > 1 {
                e = e.chunks(2).map(|p| self.fp(Fast::MinMax { max, nm }, &[p[0], p[1]])).collect();
            }
            self.vwrite(rd, e[0]);
            return Ok(());
        }
        let ty = vector_of(arr, int_type(arr.size));
        let x = self.vread(rn, ty);
        let r = match op {
            Saddlv | Uaddlv => {
                let wide = ty.with_elem(int_type(Size(arr.size.0 + 1)));
                let w = self.widen(x, wide, op == Saddlv);
                self.op(Op::Reduce(Reduce::Add, w))
            }
            Addv => self.op(Op::Reduce(Reduce::Add, x)),
            Smaxv => self.op(Op::Reduce(Reduce::SMax, x)),
            Umaxv => self.op(Op::Reduce(Reduce::UMax, x)),
            Sminv => self.op(Op::Reduce(Reduce::SMin, x)),
            Uminv => self.op(Op::Reduce(Reduce::UMin, x)),
            _ => unreachable!(),
        };
        self.vwrite(rd, r);
        Ok(())
    }

    // ---- shifts by an immediate

    fn shift_imm(&mut self, op: ShiftOp, shape: Shape, rd: VReg, rn: VReg, s: u32) -> Result<(), Fault> {
        use ShiftOp::*;
        let ty = shape_int(shape);
        let e = ty.elem().bits();
        let up = upper(shape) && !shape.is_scalar();
        if op.narrows() {
            let (nt, wt) = (narrow_type(shape), wide_type(shape));
            let x = self.vread(rn, wt);
            let round = matches!(op, Rshrn | Sqrshrn | Uqrshrn | Sqrshrun);
            let signed_src = matches!(op, Sqshrn | Sqrshrn | Sqshrun | Sqrshrun);
            let v = self.rshr_k(x, s, signed_src, round);
            let r = match op {
                Shrn | Rshrn => self.op(Op::Cast(Cast::Trunc, nt, v)),
                Sqshrn | Sqrshrn => self.sat_narrow(v, true, true),
                Uqshrn | Uqrshrn => self.sat_narrow(v, false, false),
                _ => self.sat_narrow(v, true, false),
            };
            self.vwrite_part(rd, r, up);
            return Ok(());
        }
        if op.widens() {
            let (nt, wt) = (narrow_type(shape), wide_type(shape));
            let x = self.vread_part(rn, nt, upper(shape));
            let w = self.widen(x, wt, op == Sshll);
            let r = self.shl_k(w, s);
            self.vwrite(rd, r);
            return Ok(());
        }
        if matches!(op, Scvtf | Ucvtf | Fcvtzs | Fcvtzu) {
            float_elem(e, false)?;
            let fty = shape_float(shape);
            let r = if matches!(op, Scvtf | Ucvtf) {
                let x = self.vread(rn, ty);
                self.inttof(x, s, fty, op == Ucvtf)
            } else {
                let x = self.vread(rn, fty);
                self.fp(Fast::ToInt { fbits: s, round: None, to: ty, unsigned: op == Fcvtzu }, &[x])
            };
            self.vwrite(rd, r);
            return Ok(());
        }
        let x = self.vread(rn, ty);
        let r = match op {
            Sshr | Ushr | Srshr | Urshr => self.rshr_k(x, s, matches!(op, Sshr | Srshr), matches!(op, Srshr | Urshr)),
            Ssra | Usra | Srsra | Ursra => {
                let v = self.rshr_k(x, s, matches!(op, Ssra | Srsra), matches!(op, Srsra | Ursra));
                let d = self.vread(rd, ty);
                self.bin(BinOp::Add, d, v)
            }
            Shl => self.shl_k(x, s),
            Sli => {
                let d = self.vread(rd, ty);
                let keep = self.k_like(d, !(umax(e) << s) & umax(e));
                let kept = self.bin(BinOp::And, d, keep);
                let v = self.shl_k(x, s);
                self.bin(BinOp::Or, kept, v)
            }
            Sri => {
                let d = self.vread(rd, ty);
                if s >= e {
                    d
                } else {
                    let keep = self.k_like(d, !(umax(e) >> s) & umax(e));
                    let kept = self.bin(BinOp::And, d, keep);
                    let v = self.lshr_k(x, s);
                    self.bin(BinOp::Or, kept, v)
                }
            }
            Sqshl | Uqshl | Sqshlu => {
                let l = self.shl_k(x, s);
                // (saturated where shifting back loses what went out)
                let back = self.shr_k(l, s, op == Sqshl);
                let over = self.cmp(Pred::Ne, back, x);
                match op {
                    Sqshl | Uqshl => self.saturate_left(x, l, over, op == Sqshl),
                    _ => {
                        let zero = self.k_like(x, 0);
                        let neg = self.cmp(Pred::Slt, x, zero);
                        let bad = self.bin(BinOp::Or, over, neg);
                        self.saturated(bad);
                        let mx = self.k_like(x, umax(e));
                        let limit = self.select(neg, zero, mx);
                        self.select(bad, limit, l)
                    }
                }
            }
            _ => unreachable!("a narrowing, widening or conversion form: {op:?}"),
        };
        self.vwrite(rd, r);
        Ok(())
    }

    // ---- by element

    fn by_element(&mut self, op: ElemOp, shape: Shape, rd: VReg, rn: VReg, rm: VReg, index: u8) -> Result<(), Fault> {
        use ElemOp::*;
        if matches!(op, Sdot | Udot) {
            let word = self.vlane(rm, Type::I32, index);
            let words = self.op(Op::Splat(shape_int(shape), word));
            let m = self.op(Op::Bitcast(dot_bytes(shape), words));
            self.dot(shape, rd, rn, m, op == Sdot);
            return Ok(());
        }
        let fp = matches!(op, Fmla | Fmls | Fmul | Fmulx);
        if fp {
            float_elem(shape.size().bytes() * 8, matches!(op, Fmul | Fmulx))?;
            let ty = shape_float(shape);
            let x = self.vlane(rm, ty.elem(), index);
            let m = if ty.is_vector() { self.op(Op::Splat(ty, x)) } else { x };
            let n = self.vread(rn, ty);
            let r = match op {
                Fmul => self.fp(Fast::Arith(FBinOp::Mul), &[n, m]),
                Fmulx => self.fp(Fast::MulX, &[n, m]),
                _ => {
                    let d = self.vread(rd, ty);
                    let n = if op == Fmls { self.fneg(n) } else { n };
                    self.fp(Fast::MulAdd, &[d, n, m])
                }
            };
            self.vwrite(rd, r);
            return Ok(());
        }
        if op.widens() {
            let (nt, wt) = (narrow_type(shape), wide_type(shape));
            let signed = matches!(op, Smlal | Smlsl | Smull | Sqdmlal | Sqdmlsl | Sqdmull);
            let x = self.vlane(rm, nt.elem(), index);
            let m = if nt.is_vector() { self.op(Op::Splat(nt, x)) } else { x };
            let n = self.vread_part(rn, nt, upper(shape));
            let (n, m) = (self.widen(n, wt, signed), self.widen(m, wt, signed));
            let kind = match op {
                Smull | Umull => WideMul::Mul,
                Smlal | Umlal => WideMul::Acc,
                Smlsl | Umlsl => WideMul::Sub,
                Sqdmull => WideMul::QMul,
                Sqdmlal => WideMul::QAcc,
                _ => WideMul::QSub,
            };
            let r = self.widening_mul(kind, rd, wt, n, m);
            self.vwrite(rd, r);
            return Ok(());
        }
        let ty = shape_int(shape);
        let x = self.vlane(rm, ty.elem(), index);
        let m = if ty.is_vector() { self.op(Op::Splat(ty, x)) } else { x };
        let n = self.vread(rn, ty);
        let r = match op {
            Mul => self.bin(BinOp::Mul, n, m),
            Mla | Mls => {
                let d = self.vread(rd, ty);
                let p = self.bin(BinOp::Mul, n, m);
                self.bin(if op == Mla { BinOp::Add } else { BinOp::Sub }, d, p)
            }
            Sqdmulh => self.qdmulh(n, m, false),
            Sqrdmulh => self.qdmulh(n, m, true),
            Sqrdmlah | Sqrdmlsh => {
                let d = self.vread(rd, ty);
                self.qrdmlah(d, n, m, op == Sqrdmlsh)
            }
            _ => unreachable!("a widening, FP or dot product form: {op:?}"),
        };
        self.vwrite(rd, r);
        Ok(())
    }
}
