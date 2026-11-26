//! The scalar floating-point instructions: data processing, compares, selects, immediates, conversions, FMOV.

use crate::fp::Env;
use crate::fpfast::Fast;
use crate::vreg::float_type;
use crate::{Fault, Sem};
use a64::{FmovPart, Fp1Op, Fp2Op, Fp3Op, Insn, Round};
use mir::{FBinOp, FPred, FUnOp, Type, UnOp, Value};

/// Why an FP16 form is not covered: a fused multiply-add or step, an estimate, or a fixed-point conversion of halves
/// (each would need more than a single's exact result: the guest stops there).
pub(crate) const HALF: &str = "an FP16 fused, estimate or fixed-point form";

/// The float type of elements of `bits` bits (16, 32 or 64), unless they are halves and the form is not covered for
/// them (`half` false): the one gate of the FP16 forms left out.
pub(crate) fn float_elem(bits: u32, half: bool) -> Result<Type, Fault> {
    match bits {
        16 if !half => Err(Fault::Unsupported(HALF)),
        _ => Ok(Type::float(bits).expect("16, 32 or 64 bits")),
    }
}

/// The rounding of an explicit-rounding form as a [`FUnOp`] (`None`: towards zero, the conversions' own). FRINTX and
/// FRINTI round as FPCR says, which here is to nearest.
pub(crate) fn rounding(round: Round) -> Option<FUnOp> {
    match round {
        Round::Nearest | Round::Exact | Round::Current => Some(FUnOp::RoundEven),
        Round::Plus => Some(FUnOp::Ceil),
        Round::Minus => Some(FUnOp::Floor),
        Round::Zero => None,
        Round::Away => Some(FUnOp::Round),
    }
}

impl Sem<'_> {
    pub(crate) fn scalar_fp(&mut self, insn: &Insn) -> Result<(), Fault> {
        match *insn {
            Insn::Fp1 { op, ty, rd, rn } => {
                let t = float_type(ty);
                let x = self.vread(rn, t);
                let r = match op {
                    Fp1Op::Mov => x,
                    Fp1Op::Abs => self.fabs(x),
                    Fp1Op::Neg => self.fneg(x),
                    Fp1Op::Sqrt => self.fp(Fast::Sqrt, &[x]),
                    Fp1Op::Rint(round) => {
                        let op = rounding(round).unwrap_or(FUnOp::Trunc);
                        self.fp(Fast::Rint(op), &[x])
                    }
                };
                self.vwrite(rd, r);
            }
            Insn::FpCvt { from, to, rd, rn } => {
                let x = self.vread(rn, float_type(from));
                let r = self.fp(Fast::Convert(float_type(to)), &[x]);
                self.vwrite(rd, r);
            }
            Insn::Fp2 { op, ty, rd, rn, rm } => {
                let t = float_type(ty);
                let (a, b) = (self.vread(rn, t), self.vread(rm, t));
                let r = match op {
                    Fp2Op::Mul => self.fp(Fast::Arith(FBinOp::Mul), &[a, b]),
                    Fp2Op::Div => self.fp(Fast::Arith(FBinOp::Div), &[a, b]),
                    Fp2Op::Add => self.fp(Fast::Arith(FBinOp::Add), &[a, b]),
                    Fp2Op::Sub => self.fp(Fast::Arith(FBinOp::Sub), &[a, b]),
                    Fp2Op::Max => self.fp(Fast::MinMax { max: true, nm: false }, &[a, b]),
                    Fp2Op::Min => self.fp(Fast::MinMax { max: false, nm: false }, &[a, b]),
                    Fp2Op::MaxNm => self.fp(Fast::MinMax { max: true, nm: true }, &[a, b]),
                    Fp2Op::MinNm => self.fp(Fast::MinMax { max: false, nm: true }, &[a, b]),
                    Fp2Op::NMul => {
                        let p = self.fp(Fast::Arith(FBinOp::Mul), &[a, b]);
                        self.fneg(p)
                    }
                };
                self.vwrite(rd, r);
            }
            Insn::Fp3 { op, ty, rd, rn, rm, ra } => {
                let t = float_elem(ty.bits(), false)?;
                let (n, m, a) = (self.vread(rn, t), self.vread(rm, t), self.vread(ra, t));
                let (a, n) = match op {
                    Fp3Op::Madd => (a, n),
                    Fp3Op::Msub => (a, self.fneg(n)),
                    Fp3Op::Nmadd => (self.fneg(a), self.fneg(n)),
                    Fp3Op::Nmsub => (self.fneg(a), n),
                };
                let r = self.fp(Fast::MulAdd, &[a, n, m]);
                self.vwrite(rd, r);
            }
            Insn::FpCmp { ty, rn, rm, .. } => {
                let t = float_type(ty);
                let env = self.fp_env();
                let a = self.vread(rn, t);
                let b = match rm {
                    Some(rm) => self.vread(rm, t),
                    None => self.splat_k(t, 0),
                };
                let flags = self.fcmp_flags(env, a, b);
                self.set_nzcv(flags[0], flags[1], flags[2], flags[3]);
            }
            Insn::FpCondCmp { ty, rn, rm, nzcv, cond, .. } => {
                let t = float_type(ty);
                let env = self.fp_env();
                let holds = self.cond(cond);
                let (a, b) = (self.vread(rn, t), self.vread(rm, t));
                let flags = self.fcmp_flags(env, a, b);
                let mut out = [flags[0]; 4];
                for (i, f) in flags.into_iter().enumerate() {
                    let k = self.konst(Type::I1, u128::from(nzcv >> (3 - i) & 1));
                    out[i] = self.select(holds, f, k);
                }
                self.set_nzcv(out[0], out[1], out[2], out[3]);
            }
            Insn::FpCsel { ty, rd, rn, rm, cond } => {
                let t = float_type(ty).as_int();
                let holds = self.cond(cond);
                let (a, b) = (self.vread(rn, t), self.vread(rm, t));
                let r = self.select(holds, a, b);
                self.vwrite(rd, r);
            }
            Insn::FpImm { ty, rd, imm8 } => {
                let t = float_type(ty).as_int();
                let k = self.konst(t, u128::from(a64::fp_expand_imm(imm8, ty)));
                self.vwrite(rd, k);
            }
            Insn::FpToInt { ty, sf, round, unsigned, fbits, rd, rn } => {
                let t = float_elem(ty.bits(), fbits == 0)?;
                let x = self.vread(rn, t);
                let to = if sf { Type::I64 } else { Type::I32 };
                let r = self.fp(Fast::ToInt { fbits: u32::from(fbits), round: rounding(round), to, unsigned }, &[x]);
                self.write(rd, r, false);
            }
            Insn::IntToFp { ty, sf, unsigned, fbits, rd, rn } => {
                let t = float_elem(ty.bits(), fbits == 0)?;
                let x = self.read(rn, sf, false);
                let r = self.inttof(x, u32::from(fbits), t, unsigned);
                self.vwrite(rd, r);
            }
            Insn::FmovGen { part, to_general, gpr, fpr } => {
                let bits = match part {
                    FmovPart::WH | FmovPart::XH => Type::I16,
                    FmovPart::WS => Type::I32,
                    FmovPart::XD | FmovPart::XTop => Type::I64,
                };
                match (to_general, part) {
                    (true, FmovPart::XTop) => {
                        let v = self.vread_hi(fpr, Type::I64);
                        self.write(gpr, v, false);
                    }
                    (true, _) => {
                        let v = self.vread(fpr, bits);
                        self.write(gpr, v, false);
                    }
                    (false, FmovPart::XTop) => {
                        let x = self.x(gpr);
                        self.vwrite_hi(fpr, x);
                    }
                    (false, _) => {
                        let x = self.x(gpr);
                        let v = self.zext(x, bits);
                        self.vwrite(fpr, v);
                    }
                }
            }
            _ => unreachable!("not a scalar floating-point instruction: {insn}"),
        }
        Ok(())
    }

    /// FPCompare's flags (N, Z, C, V): less 1000, equal 0110, greater 0010, unordered 0011.
    fn fcmp_flags(&mut self, env: Env, a: Value, b: Value) -> [Value; 4] {
        let lt = self.fcompare(env, FPred::Olt, a, b);
        let eq = self.fcompare(env, FPred::Oeq, a, b);
        let uno = self.fcompare(env, FPred::Uno, a, b);
        let c = self.un(UnOp::Not, lt);
        [lt, eq, c, uno]
    }
}
