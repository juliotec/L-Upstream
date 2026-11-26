//! The float operations' fast paths: the host's IEEE operation inline, and the exact semantics (fp.rs) out of line.
//!
//! **Why it is exact.** The lifted code runs on x86-64 under the host's default MXCSR: round to nearest, DAZ and FTZ
//! clear, every exception masked (nothing in the host changes it; host/hos_setjmp.c saves and restores it as it is).
//! There, an IEEE operation's result is the Arm ARM's for the same operands under this model (fp.rs) except
//! - where it is a NaN: Arm's NaN is chosen from the operands (or is the default NaN, under DN or for an invalid
//!   operation), the host's is not; every NaN operand makes a NaN result, so testing the result covers them;
//! - under FPCR.FZ, where an operand or the result is subnormal (FZ flushes single and double precision only).
//!
//! So each operation is an [`Op::Checked`]: its fast value the host's operation, its slow condition a NaN result or a
//! subnormal operand or result (in any lane of a vector), and its helper the exact semantics, written once per module
//! and called with the operands and FPCR. FPCR.DN changes only NaN results: the helper reads it.
//!
//! The check does not read FZ: a subnormal takes the slow path with FZ clear too, where the fast value would have
//! been right. That is rare (and slow on the host's own float unit anyway), and it keeps the check short: one
//! `is.fpclass` per operand, one for the result (NaN or subnormal), ORed, and for a vector one reduction.

use crate::Sem;
use crate::fp::Env;
use mir::{BinOp, FBinOp, FClass, FPred, FUnOp, Function, Helper, HelperId, Loc, Op, Reduce, Term, Type, Value};

/// A float operation with a fast path: what [`Sem::fp`] computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Fast {
    /// FPAdd, FPSub, FPMul, FPDiv of `[a, b]` ([`Sem::farith`]).
    Arith(FBinOp),
    /// FPMulX of `[a, b]`.
    MulX,
    /// FPMulAdd of `[addend, a, b]`.
    MulAdd,
    /// FPMax, FPMin, FPMaxNum, FPMinNum of `[a, b]`.
    MinMax {
        /// The larger (else the smaller).
        max: bool,
        /// The Num forms.
        nm: bool,
    },
    /// FPSqrt of `[x]`.
    Sqrt,
    /// FPRoundInt of `[x]` with this rounding.
    Rint(FUnOp),
    /// FPConvert of `[x]` to this float type.
    Convert(Type),
    /// FPToFixed of `[x]` ([`Sem::ftoint`]'s operands).
    ToInt {
        /// The fraction bits.
        fbits: u32,
        /// The rounding (`None`: towards zero).
        round: Option<FUnOp>,
        /// The integer type.
        to: Type,
        /// Unsigned (else signed).
        unsigned: bool,
    },
    /// FRECPS or (`rsqrt`) FRSQRTS of `[n, m]`.
    Step {
        /// FRSQRTS.
        rsqrt: bool,
    },
}

/// A type as a helper's name spells it: f32, v4f32, i64.
fn spell(ty: Type) -> String {
    let scalar = |t: Type| if t.is_float() { format!("f{}", t.bits()) } else { format!("i{}", t.bits()) };
    match ty {
        Type::Vec(n, e) => format!("v{n}{}", scalar(e.scalar())),
        t => scalar(t),
    }
}

fn unop_name(op: FUnOp) -> &'static str {
    match op {
        FUnOp::Sqrt => "sqrt",
        FUnOp::RoundEven => "roundeven",
        FUnOp::Round => "round",
        FUnOp::Floor => "floor",
        FUnOp::Ceil => "ceil",
        FUnOp::Trunc => "trunc",
    }
}

impl Fast {
    /// Its helper's name for operands of `params` (the operands', then FPCR's): every field of it is in the name, so
    /// that two helpers of one name compute the same.
    pub(crate) fn helper_name(self, params: &[Type]) -> String {
        let what = match self {
            Fast::Arith(op) => format!("{op:?}").to_lowercase(),
            Fast::MulX => "mulx".into(),
            Fast::MulAdd => "muladd".into(),
            Fast::MinMax { max, nm } => format!("{}{}", if max { "max" } else { "min" }, if nm { "nm" } else { "" }),
            Fast::Sqrt => "sqrt".into(),
            Fast::Rint(op) => format!("rint.{}", unop_name(op)),
            Fast::Convert(to) => format!("cvt.{}", spell(to)),
            Fast::ToInt { fbits, round, to, unsigned } => format!(
                "toint.{}.{}.{fbits}.{}",
                spell(to),
                if unsigned { "u" } else { "s" },
                round.map_or("zero", unop_name)
            ),
            Fast::Step { rsqrt } => if rsqrt { "rsqrts" } else { "recps" }.into(),
        };
        let params: Vec<String> = params.iter().map(|&t| spell(t)).collect();
        format!("lifter.fp.{what}.{}", params.join("."))
    }
}

impl Sem<'_> {
    /// `kind` of `ops`: the host's operation unless its result is a NaN or an operand or the result is subnormal,
    /// else the exact semantics' (an out-of-line helper's).
    pub(crate) fn fp(&mut self, kind: Fast, ops: &[Value]) -> Value {
        let fpcr = self.get(Loc::Fpcr);
        let fast = self.quick(kind, ops);
        // (the values FZ would flush: single and double operands, and the result where it can be subnormal)
        let flushable = |s: &Self, x: Value| matches!(s.ty(x).elem(), Type::F32 | Type::F64);
        let result_flushed = flushable(self, fast)
            && match kind {
                Fast::Arith(_) | Fast::MulX | Fast::MulAdd | Fast::Step { .. } => true,
                Fast::Convert(to) => to.elem().bits() < self.ty(ops[0]).elem().bits(),
                Fast::MinMax { .. } | Fast::Sqrt | Fast::Rint(_) | Fast::ToInt { .. } => false,
            };
        let mut tests = Vec::new();
        for &x in ops {
            if flushable(self, x) {
                tests.push(self.op(Op::FClass(FClass::Subnormal, x)));
            }
        }
        if result_flushed {
            tests.push(self.op(Op::FClass(FClass::NanOrSubnormal, fast)));
        } else if self.ty(fast).is_float() {
            tests.push(self.op(Op::FCmp(FPred::Uno, fast, fast)));
        }
        let Some((&first, rest)) = tests.split_first() else { return fast };
        let mut any = first;
        for &t in rest {
            any = self.bin(BinOp::Or, any, t);
        }
        let slow = self.any_lane(any);
        let helper = self.fp_helper(kind, ops);
        let mut args = ops.to_vec();
        args.push(fpcr);
        self.op(Op::Checked { fast, slow, helper, args })
    }

    /// Whether any lane of the lane mask `m` is set (`m` itself, of a scalar).
    fn any_lane(&mut self, m: Value) -> Value {
        if self.ty(m).is_vector() { self.op(Op::Reduce(Reduce::Or, m)) } else { m }
    }

    /// `kind`'s fast value: the host's operation, the exact one's wherever [`Sem::fp`]'s check passes.
    fn quick(&mut self, kind: Fast, ops: &[Value]) -> Value {
        match kind {
            Fast::Arith(op) => self.host_bin(op, ops[0], ops[1]),
            Fast::MulX => self.host_bin(FBinOp::Mul, ops[0], ops[1]),
            Fast::MulAdd => self.op(Op::Fma(ops[1], ops[2], ops[0])),
            Fast::MinMax { max, .. } => {
                // (without NaNs FPMaxNum is FPMax, and FPMax is IEEE's maximum: −0 below +0)
                let op = if max { FBinOp::Maximum } else { FBinOp::Minimum };
                self.host_bin(op, ops[0], ops[1])
            }
            Fast::Sqrt => self.host_un(FUnOp::Sqrt, ops[0]),
            Fast::Rint(op) => self.host_un(op, ops[0]),
            Fast::Convert(to) => self.fcast_float(ops[0], to),
            Fast::ToInt { fbits, round, to, unsigned } => self.ftoint_core(ops[0], fbits, round, to, unsigned),
            Fast::Step { rsqrt } => {
                let neg = self.fneg(ops[0]);
                self.step_core(neg, ops[1], rsqrt)
            }
        }
    }

    /// `kind`'s exact semantics (fp.rs, simd.rs), under `env`.
    fn exact(&mut self, env: Env, kind: Fast, ops: &[Value]) -> Value {
        match kind {
            Fast::Arith(op) => self.farith(env, op, ops[0], ops[1]),
            Fast::MulX => self.fmulx(env, ops[0], ops[1]),
            Fast::MulAdd => self.fmuladd(env, ops[0], ops[1], ops[2]),
            Fast::MinMax { max, nm } => self.fminmax(env, ops[0], ops[1], max, nm),
            Fast::Sqrt => self.fsqrt(env, ops[0]),
            Fast::Rint(op) => self.frint(env, op, ops[0]),
            Fast::Convert(to) => self.fconvert(env, ops[0], to),
            Fast::ToInt { fbits, round, to, unsigned } => self.ftoint(env, ops[0], fbits, round, to, unsigned),
            Fast::Step { rsqrt } => self.step(env, ops[0], ops[1], rsqrt),
        }
    }

    /// The helper computing `kind` of operands like `ops` exactly, from them and FPCR: the function's own, made once.
    fn fp_helper(&mut self, kind: Fast, ops: &[Value]) -> HelperId {
        let mut params: Vec<Type> = ops.iter().map(|&x| self.ty(x)).collect();
        params.push(Type::I64);
        let name = kind.helper_name(&params);
        if let Some(id) = self.f.helper_named(&name) {
            return id;
        }
        let mut body = Function::new(0);
        let b = body.start;
        let values: Vec<Value> =
            params.iter().enumerate().map(|(i, &ty)| body.push(b, Op::Param { index: i as u8, ty })).collect();
        let (args, fpcr) = values.split_at(ops.len());
        let mut s = Sem::new(&mut body, b, self.pc);
        let env = s.env_of(fpcr[0]);
        let result = s.exact(env, kind, args);
        body.terminate(b, Term::Return);
        self.f.add_helper(Helper { name, params, body, result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_differ_where_the_computations_do() {
        let p = [Type::F32, Type::F32, Type::I64];
        let kinds = [
            Fast::Arith(FBinOp::Add),
            Fast::Arith(FBinOp::Sub),
            Fast::Arith(FBinOp::Mul),
            Fast::Arith(FBinOp::Div),
            Fast::MulX,
            Fast::MulAdd,
            Fast::MinMax { max: true, nm: false },
            Fast::MinMax { max: false, nm: false },
            Fast::MinMax { max: true, nm: true },
            Fast::MinMax { max: false, nm: true },
            Fast::Sqrt,
            Fast::Rint(FUnOp::Floor),
            Fast::Rint(FUnOp::Ceil),
            Fast::Convert(Type::F64),
            Fast::Convert(Type::F16),
            Fast::ToInt { fbits: 0, round: None, to: Type::I32, unsigned: false },
            Fast::ToInt { fbits: 0, round: None, to: Type::I32, unsigned: true },
            Fast::ToInt { fbits: 3, round: None, to: Type::I32, unsigned: true },
            Fast::ToInt { fbits: 3, round: Some(FUnOp::Floor), to: Type::I32, unsigned: true },
            Fast::ToInt { fbits: 3, round: Some(FUnOp::Floor), to: Type::I64, unsigned: true },
            Fast::Step { rsqrt: false },
            Fast::Step { rsqrt: true },
        ];
        let names: std::collections::BTreeSet<String> = kinds.iter().map(|k| k.helper_name(&p)).collect();
        assert_eq!(names.len(), kinds.len());
        let v = [Type::Vec(4, mir::Elem::F32), Type::Vec(4, mir::Elem::F32), Type::I64];
        assert_ne!(Fast::Sqrt.helper_name(&p), Fast::Sqrt.helper_name(&v));
        assert_eq!(Fast::Arith(FBinOp::Add).helper_name(&v), "lifter.fp.add.v4f32.v4f32.i64");
    }
}
