//! Floating point as the Arm ARM defines it (FPAdd, FPMulAdd, FPMax, FPProcessNaNs ... of shared/functions/float),
//! built from IEEE arithmetic rounded to nearest. Every helper takes floats of any shape (a scalar or a vector of H, S
//! or D) and works lane by lane.
//!
//! **The model** (the host's, which the game runs under):
//! - rounding is to nearest, ties to even: FPCR.RMode is not read (the explicit roundings of FRINT* and FCVT*, and
//!   FCVTXN's round to odd, are the instructions' own);
//! - FPCR.FZ is read at each instruction: single and double operands that are subnormal count as zeros of their sign,
//!   and so do results that are subnormal once rounded (the Arm ARM flushes a result tiny before rounding: one that
//!   rounds up to the smallest normal is that normal here, not zero);
//! - FPCR.DN is read: with it, every NaN result is the default NaN;
//! - NaNs otherwise are Arm's: a signalling operand first (in operand order: a multiply-add's addend first), quieted,
//!   else the first quiet one; an invalid operation without a NaN operand gives the default NaN (0x7fc00000, positive,
//!   where x86 makes a negative one);
//! - the cumulative exception flags of FPSR (IOC, DZC, OFC, UFC, IXC, IDC) are not raised; QC, the saturation flag,
//!   is (the saturating integer operations');
//! - FEAT_FP16's arithmetic on halves (add, subtract, multiply, divide, square root, roundings, min and max, compares,
//!   conversions) is computed through single precision, which is exact ([`Sem::host_bin`]); its fused forms (FMADD,
//!   FMLA, FRECPS ...), estimates (FRECPE, FRSQRTE, FRECPX) and fixed-point conversions are not covered (the guest stops
//!   at them); FPCR.AHP and FZ16 are not modelled (halves are IEEE and never flushed).

use crate::Sem;
use mir::{BinOp, FBinOp, FPred, FUnOp, Loc, Op, Pred, Type, UnOp, Value};

/// The FPCR bits an instruction reads: flush to zero and default NaN, each an `i1`.
#[derive(Clone, Copy)]
pub(crate) struct Env {
    /// FPCR.FZ.
    pub fz: Value,
    /// FPCR.DN.
    pub dn: Value,
}

/// A float format's fields, as masks of its bits.
#[derive(Clone, Copy)]
pub(crate) struct Fmt {
    /// Its width.
    pub bits: u32,
    /// The fraction's width.
    pub frac: u32,
}

impl Fmt {
    /// The format of `ty`'s elements.
    pub fn of(ty: Type) -> Fmt {
        let bits = ty.elem().bits();
        let frac = match bits {
            16 => 10,
            32 => 23,
            _ => 52,
        };
        Fmt { bits, frac }
    }

    /// The exponent's width.
    pub fn exp_bits(self) -> u32 {
        self.bits - 1 - self.frac
    }

    /// The sign bit.
    pub fn sign(self) -> u128 {
        1 << (self.bits - 1)
    }

    /// The exponent field.
    pub fn exp(self) -> u128 {
        ((1u128 << self.exp_bits()) - 1) << self.frac
    }

    /// The fraction field.
    pub fn frac_mask(self) -> u128 {
        (1u128 << self.frac) - 1
    }

    /// A NaN's quiet bit.
    pub fn quiet(self) -> u128 {
        1 << (self.frac - 1)
    }

    /// The default NaN: positive, quiet, its payload zero.
    pub fn default_nan(self) -> u128 {
        self.exp() | self.quiet()
    }

    /// Plus infinity.
    pub fn inf(self) -> u128 {
        self.exp()
    }

    /// The exponent's bias.
    pub fn bias(self) -> u128 {
        (1 << (self.exp_bits() - 1)) - 1
    }

    /// The value 2^n (n within the normal range).
    pub fn pow2(self, n: i32) -> u128 {
        ((self.bias() as i64 + i64::from(n)) as u128) << self.frac
    }
}

impl Sem<'_> {
    /// FPCR's FZ and DN, read for the instruction being lowered.
    pub(crate) fn fp_env(&mut self) -> Env {
        let fpcr = self.get(Loc::Fpcr);
        self.env_of(fpcr)
    }

    /// FZ and DN of the FPCR value `fpcr` (an `i64`).
    pub(crate) fn env_of(&mut self, fpcr: Value) -> Env {
        let fz = self.bit(fpcr, 24);
        let dn = self.bit(fpcr, 25);
        Env { fz, dn }
    }

    /// The format of `x`'s elements.
    pub(crate) fn fmt(&self, x: Value) -> Fmt {
        Fmt::of(self.ty(x))
    }

    /// Integer bits of `x`'s type back as floats of `ty`.
    pub(crate) fn floats_of(&mut self, ty: Type, bits: Value) -> Value {
        self.cast_to(bits, ty)
    }

    /// `x`'s bits `& mask`, compared with `want`: a lane mask.
    fn field_is(&mut self, bits: Value, mask: u128, want: u128) -> Value {
        let m = self.k_like(bits, mask);
        let masked = self.bin(BinOp::And, bits, m);
        let w = self.k_like(bits, want);
        self.cmp(Pred::Eq, masked, w)
    }

    /// Whether each lane of `x` is a NaN.
    pub(crate) fn is_nan(&mut self, x: Value) -> Value {
        self.op(Op::FCmp(FPred::Uno, x, x))
    }

    /// Whether each lane of `x` is a signalling NaN.
    pub(crate) fn is_snan(&mut self, x: Value) -> Value {
        let nan = self.is_nan(x);
        let f = self.fmt(x);
        let bits = self.as_int(x);
        let quiet = self.field_is(bits, f.quiet(), 0);
        self.bin(BinOp::And, nan, quiet)
    }

    /// Whether each lane of `x` is a quiet NaN.
    pub(crate) fn is_qnan(&mut self, x: Value) -> Value {
        let f = self.fmt(x);
        let bits = self.as_int(x);
        self.field_is(bits, f.exp() | f.quiet(), f.exp() | f.quiet())
    }

    /// Whether each lane of `x` is an infinity.
    pub(crate) fn is_inf(&mut self, x: Value) -> Value {
        let f = self.fmt(x);
        let bits = self.as_int(x);
        self.field_is(bits, f.exp() | f.frac_mask(), f.exp())
    }

    /// Whether each lane of `x` is a zero (of either sign).
    pub(crate) fn is_fzero(&mut self, x: Value) -> Value {
        let f = self.fmt(x);
        let bits = self.as_int(x);
        self.field_is(bits, !f.sign() & ((1u128 << f.bits) - 1), 0)
    }

    /// `x` with its sign bits flipped (FPNeg: no NaN is processed).
    pub(crate) fn fneg(&mut self, x: Value) -> Value {
        let f = self.fmt(x);
        let bits = self.as_int(x);
        let s = self.k_like(bits, f.sign());
        let r = self.bin(BinOp::Xor, bits, s);
        self.floats_of(self.ty(x), r)
    }

    /// `x` with its sign bits cleared (FPAbs).
    pub(crate) fn fabs(&mut self, x: Value) -> Value {
        let f = self.fmt(x);
        let bits = self.as_int(x);
        let s = self.k_like(bits, !f.sign());
        let r = self.bin(BinOp::And, bits, s);
        self.floats_of(self.ty(x), r)
    }

    /// `x` with FZ applied: under FPCR.FZ, a single or double subnormal is a zero of its sign. Half precision is not
    /// flushed (FZ16 is not modelled).
    pub(crate) fn flush(&mut self, env: Env, x: Value) -> Value {
        let f = self.fmt(x);
        if f.bits == 16 {
            return x;
        }
        let bits = self.as_int(x);
        let tiny = self.field_is(bits, f.exp(), 0);
        let s = self.k_like(bits, f.sign());
        let zero = self.bin(BinOp::And, bits, s);
        let flushed = self.select(tiny, zero, bits);
        let r = self.select(env.fz, flushed, bits);
        self.floats_of(self.ty(x), r)
    }

    /// The NaN Arm's FPProcessNaNs makes of `ops` when one of them (in each lane) is a NaN: the first signalling one,
    /// else the first quiet one, quieted; the default NaN under FPCR.DN. Integer bits.
    fn nan_of(&mut self, env: Env, ops: &[Value]) -> Value {
        let f = self.fmt(ops[0]);
        let mut quieted = Vec::new();
        for &x in ops {
            let bits = self.as_int(x);
            let q = self.k_like(bits, f.quiet());
            quieted.push(self.bin(BinOp::Or, bits, q));
        }
        let mut pick = *quieted.last().expect("an operand");
        for (i, &x) in ops.iter().enumerate().rev().skip(1) {
            let nan = self.is_nan(x);
            pick = self.select(nan, quieted[i], pick);
        }
        for (i, &x) in ops.iter().enumerate().rev() {
            let snan = self.is_snan(x);
            pick = self.select(snan, quieted[i], pick);
        }
        let dflt = self.k_like(pick, f.default_nan());
        self.select(env.dn, dflt, pick)
    }

    /// The default NaN of `x`'s type, as integer bits.
    fn default_nan_bits(&mut self, x: Value) -> Value {
        let f = self.fmt(x);
        let ty = self.ty(x).as_int();
        self.splat_k(ty, f.default_nan())
    }

    /// Whether any of `ops` is a NaN, lane by lane.
    fn any_nan(&mut self, ops: &[Value]) -> Value {
        let mut any = self.is_nan(ops[0]);
        for &x in &ops[1..] {
            let n = self.is_nan(x);
            any = self.bin(BinOp::Or, any, n);
        }
        any
    }

    /// `r`, the host's result of an operation of `ops`, with Arm's NaN where it is a NaN: the operands' NaN if they have
    /// one, else (an invalid operation) the default NaN.
    pub(crate) fn nan_fix(&mut self, env: Env, r: Value, ops: &[Value]) -> Value {
        let ty = self.ty(r);
        let from_ops = self.nan_of(env, ops);
        let dflt = self.default_nan_bits(r);
        let any = self.any_nan(ops);
        let nan = self.select(any, from_ops, dflt);
        let is = self.is_nan(r);
        let bits = self.as_int(r);
        let out = self.select(is, nan, bits);
        self.floats_of(ty, out)
    }

    /// `a op b`, the host's IEEE operation, of floats of any shape. Halves go through single precision and back, as
    /// FEAT_FP16's arithmetic is written here: that is exact, since a single (24 bits) holds a half's (11 bits) sum,
    /// difference, product, quotient or square root rounded once with room enough (24 ≥ 2 × 11 + 2) that rounding it
    /// again to a half gives the half's own correctly rounded result, and a single has the range of every such result
    /// of halves, subnormal ones included. (Maximum and minimum are exact anyway.)
    pub(crate) fn host_bin(&mut self, op: FBinOp, a: Value, b: Value) -> Value {
        let ty = self.ty(a);
        match single_of_half(ty) {
            None => self.op(Op::FBin(op, a, b)),
            Some(wide) => {
                let wa = self.op(Op::FCast(mir::FCast::Ext, wide, a));
                let wb = self.op(Op::FCast(mir::FCast::Ext, wide, b));
                let r = self.op(Op::FBin(op, wa, wb));
                self.op(Op::FCast(mir::FCast::Trunc, ty, r))
            }
        }
    }

    /// `op x`, the host's, of floats of any shape (halves through single precision, exact as in [`Sem::host_bin`]; a
    /// rounding to an integral value is exact in any format).
    pub(crate) fn host_un(&mut self, op: FUnOp, x: Value) -> Value {
        let ty = self.ty(x);
        match single_of_half(ty) {
            None => self.op(Op::FUn(op, x)),
            Some(wide) => {
                let w = self.op(Op::FCast(mir::FCast::Ext, wide, x));
                let r = self.op(Op::FUn(op, w));
                self.op(Op::FCast(mir::FCast::Trunc, ty, r))
            }
        }
    }

    /// FPAdd, FPSub, FPMul, FPDiv: the operands flushed, the result's NaN Arm's, the result flushed.
    pub(crate) fn farith(&mut self, env: Env, op: FBinOp, a: Value, b: Value) -> Value {
        let a = self.flush(env, a);
        let b = self.flush(env, b);
        let r = self.host_bin(op, a, b);
        let r = self.nan_fix(env, r, &[a, b]);
        self.flush(env, r)
    }

    /// FPMulX: FPMul, except that infinity times zero is 2.0 of the signs' product.
    pub(crate) fn fmulx(&mut self, env: Env, a: Value, b: Value) -> Value {
        let a = self.flush(env, a);
        let b = self.flush(env, b);
        let r = self.host_bin(FBinOp::Mul, a, b);
        let r = self.nan_fix(env, r, &[a, b]);
        let r = self.flush(env, r);
        let special = self.inf_times_zero(a, b);
        let f = self.fmt(a);
        let two = self.k_like(a, f.pow2(1));
        let ab = self.as_int(a);
        let bb = self.as_int(b);
        let sign = self.bin(BinOp::Xor, ab, bb);
        let sm = self.k_like(sign, f.sign());
        let sign = self.bin(BinOp::And, sign, sm);
        let two = self.as_int(two);
        let signed_two = self.bin(BinOp::Or, two, sign);
        let signed_two = self.floats_of(self.ty(a), signed_two);
        self.select(special, signed_two, r)
    }

    /// Whether `a` × `b` is infinity times zero, lane by lane.
    pub(crate) fn inf_times_zero(&mut self, a: Value, b: Value) -> Value {
        let (ia, ib) = (self.is_inf(a), self.is_inf(b));
        let (za, zb) = (self.is_fzero(a), self.is_fzero(b));
        let x = self.bin(BinOp::And, ia, zb);
        let y = self.bin(BinOp::And, za, ib);
        self.bin(BinOp::Or, x, y)
    }

    /// FPMulAdd: `addend + a × b`, rounded once; NaNs in the order addend, a, b, and a quiet NaN addend with infinity
    /// times zero the default NaN.
    pub(crate) fn fmuladd(&mut self, env: Env, addend: Value, a: Value, b: Value) -> Value {
        let addend = self.flush(env, addend);
        let a = self.flush(env, a);
        let b = self.flush(env, b);
        let r = self.op(Op::Fma(a, b, addend));
        let r = self.nan_fix(env, r, &[addend, a, b]);
        let qa = self.is_qnan(addend);
        let iz = self.inf_times_zero(a, b);
        let invalid = self.bin(BinOp::And, qa, iz);
        let dflt = self.default_nan_bits(r);
        let dflt = self.floats_of(self.ty(r), dflt);
        let r = self.select(invalid, dflt, r);
        self.flush(env, r)
    }

    /// FPMax or FPMin (`max`), or with `nm` FPMaxNum / FPMinNum (a quiet NaN beside a number gives the number).
    pub(crate) fn fminmax(&mut self, env: Env, a: Value, b: Value, max: bool, nm: bool) -> Value {
        let ty = self.ty(a);
        let mut a = self.flush(env, a);
        let mut b = self.flush(env, b);
        let f = self.fmt(a);
        if nm {
            // (FPMaxNum: a quiet NaN against anything but a quiet NaN is the infinity that loses)
            let losing = self.k_like(a, if max { f.inf() | f.sign() } else { f.inf() });
            let (qa, qb) = (self.is_qnan(a), self.is_qnan(b));
            let (nqa, nqb) = (self.un(UnOp::Not, qa), self.un(UnOp::Not, qb));
            let only_a = self.bin(BinOp::And, qa, nqb);
            let only_b = self.bin(BinOp::And, qb, nqa);
            a = self.select(only_a, losing, a);
            b = self.select(only_b, losing, b);
        }
        let pick_a = self.op(Op::FCmp(if max { FPred::Ogt } else { FPred::Olt }, a, b));
        let r = self.select(pick_a, a, b);
        // (two zeros: the AND of their signs for max, the OR for min)
        let (za, zb) = (self.is_fzero(a), self.is_fzero(b));
        let zeros = self.bin(BinOp::And, za, zb);
        let (ab, bb) = (self.as_int(a), self.as_int(b));
        let joined = self.bin(if max { BinOp::And } else { BinOp::Or }, ab, bb);
        let rb = self.as_int(r);
        let r = self.select(zeros, joined, rb);
        let r = self.floats_of(ty, r);
        let any = self.any_nan(&[a, b]);
        let nan = self.nan_of(env, &[a, b]);
        let nan = self.floats_of(ty, nan);
        self.select(any, nan, r)
    }

    /// FPSqrt.
    pub(crate) fn fsqrt(&mut self, env: Env, x: Value) -> Value {
        let x = self.flush(env, x);
        let r = self.host_un(FUnOp::Sqrt, x);
        let r = self.nan_fix(env, r, &[x]);
        self.flush(env, r)
    }

    /// FPRoundInt with an explicit rounding (`op`: RoundEven, Round, Floor, Ceil or Trunc).
    pub(crate) fn frint(&mut self, env: Env, op: FUnOp, x: Value) -> Value {
        let x = self.flush(env, x);
        let r = self.host_un(op, x);
        self.nan_fix(env, r, &[x])
    }

    /// A comparison of floats as FPCompareEQ / GE / GT (the operands flushed): a lane mask.
    pub(crate) fn fcompare(&mut self, env: Env, pred: FPred, a: Value, b: Value) -> Value {
        let a = self.flush(env, a);
        let b = self.flush(env, b);
        self.op(Op::FCmp(pred, a, b))
    }

    /// The quieted NaN of `x` (with its payload) converted to `to`'s format: FPConvertNaN; the default NaN under DN.
    pub(crate) fn convert_nan(&mut self, env: Env, x: Value, to: Type) -> Value {
        let (from_f, to_f) = (self.fmt(x), Fmt::of(to));
        let bits = self.as_int(x);
        let ity = to.as_int();
        let sign_m = self.k_like(bits, from_f.sign());
        let sign = self.bin(BinOp::And, bits, sign_m);
        let frac_m = self.k_like(bits, from_f.frac_mask());
        let frac = self.bin(BinOp::And, bits, frac_m);
        let (sign, frac) = if to_f.bits > from_f.bits {
            let s = self.zext(sign, ity);
            let s = self.shl_k(s, to_f.bits - from_f.bits);
            let fr = self.zext(frac, ity);
            (s, self.shl_k(fr, to_f.frac - from_f.frac))
        } else {
            let s = self.lshr_k(sign, from_f.bits - to_f.bits);
            let s = self.zext(s, ity);
            let fr = self.lshr_k(frac, from_f.frac - to_f.frac);
            (s, self.zext(fr, ity))
        };
        let nan = self.splat_k(ity, to_f.exp() | to_f.quiet());
        let r = self.bin(BinOp::Or, sign, frac);
        let r = self.bin(BinOp::Or, r, nan);
        let dflt = self.splat_k(ity, to_f.default_nan());
        let r = self.select(env.dn, dflt, r);
        self.floats_of(to, r)
    }

    /// FPConvert between float formats (rounding to nearest when narrowing). FZ applies to a single or double source
    /// and result, not to half precision.
    pub(crate) fn fconvert(&mut self, env: Env, x: Value, to: Type) -> Value {
        let x = self.flush(env, x);
        let r = self.fcast_float(x, to);
        let r = self.flush(env, r);
        let nan = self.convert_nan(env, x, to);
        let is = self.is_nan(x);
        self.select(is, nan, r)
    }

    /// `x` converted to the float type `to`, wider or narrower (rounded to nearest): the host's conversion.
    pub(crate) fn fcast_float(&mut self, x: Value, to: Type) -> Value {
        let wider = to.elem().bits() > self.ty(x).elem().bits();
        self.op(Op::FCast(if wider { mir::FCast::Ext } else { mir::FCast::Trunc }, to, x))
    }

    /// FPToFixed: `x` × 2^`fbits` rounded by `round` (a [`FUnOp`] rounding, or `None` for towards zero) to an integer
    /// of `to` (its shape `x`'s), saturating, a NaN to 0.
    pub(crate) fn ftoint(
        &mut self,
        env: Env,
        x: Value,
        fbits: u32,
        round: Option<FUnOp>,
        to: Type,
        unsigned: bool,
    ) -> Value {
        let x = self.flush(env, x);
        self.ftoint_core(x, fbits, round, to, unsigned)
    }

    /// [`Sem::ftoint`] of an operand already flushed.
    pub(crate) fn ftoint_core(
        &mut self,
        mut x: Value,
        fbits: u32,
        round: Option<FUnOp>,
        to: Type,
        unsigned: bool,
    ) -> Value {
        if fbits != 0 {
            let f = self.fmt(x);
            let scale = self.k_like(x, f.pow2(fbits as i32));
            x = self.op(Op::FBin(FBinOp::Mul, x, scale));
        }
        if let Some(r) = round {
            x = self.host_un(r, x);
        }
        let cast = if unsigned { mir::FCast::ToUnsigned } else { mir::FCast::ToSigned };
        self.op(Op::FCast(cast, to, x))
    }

    /// FixedToFP: the integer `x` (signed unless `unsigned`) ÷ 2^`fbits` as a float of `to`, rounded to nearest.
    ///
    /// FZ needs no flush here: no result is subnormal. A nonzero integer of 64 bits or fewer, as a float, is at least
    /// 1 in magnitude, and ÷ 2^`fbits` (at most 64), exact, at least 2^-64: a normal single or double. (A half is
    /// not flushed: FZ16 is not modelled.)
    pub(crate) fn inttof(&mut self, x: Value, fbits: u32, to: Type, unsigned: bool) -> Value {
        let cast = if unsigned { mir::FCast::FromUnsigned } else { mir::FCast::FromSigned };
        let mut r = self.op(Op::FCast(cast, to, x));
        if fbits != 0 {
            let f = Fmt::of(to);
            let scale = self.splat_k(to, f.pow2(-(fbits as i32)));
            r = self.op(Op::FBin(FBinOp::Mul, r, scale));
        }
        r
    }
}

/// The single-precision shape `ty`'s halves are computed in (`None`: `ty` is no half).
fn single_of_half(ty: Type) -> Option<Type> {
    (ty.elem() == Type::F16).then(|| ty.with_elem(Type::F32))
}
