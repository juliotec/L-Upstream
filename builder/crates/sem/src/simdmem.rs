//! The Advanced SIMD structure loads and stores: LD1 ... LD4, LD1R ... LD4R, ST1 ... ST4.

use crate::mem::space;
use crate::vreg::{int_type, vector_of};
use crate::{Fault, Sem};
use a64::{Insn, PostIndex, Structure};
use mir::{BinOp, Elem, Op, Type, Value};

impl Sem<'_> {
    pub(crate) fn structure(&mut self, insn: &Insn) -> Result<(), Fault> {
        let Insn::SimdLdSt { load, what, rt, rn, post } = *insn else {
            unreachable!("not a structure load or store: {insn}")
        };
        let base = self.xsp(rn);
        let stack = rn.is_31();
        let at = |s: &mut Self, offset: u32| {
            if offset == 0 {
                return base;
            }
            let k = s.k64(u64::from(offset));
            s.bin(BinOp::Add, base, k)
        };
        // (the post-index register is read before any register is written)
        let step = match post {
            Some(PostIndex::Imm(n)) => Some(self.k64(u64::from(n))),
            Some(PostIndex::Reg(m)) => Some(self.x(m)),
            None => None,
        };
        match what {
            Structure::Multiple { arr, regs, selem } => {
                let ty = vector_of(arr, int_type(arr.size));
                let bytes = arr.bits() / 8;
                if selem == 1 {
                    for r in 0..regs {
                        let a = at(self, u32::from(r) * bytes);
                        if load {
                            let v = self.op(Op::Load { ty, addr: a, order: None });
                            self.vwrite(rt.plus(r), v);
                        } else {
                            let v = self.vread(rt.plus(r), ty);
                            self.op(Op::Store { addr: a, value: v, space: space(stack), order: None });
                        }
                    }
                } else {
                    let lanes = arr.lanes() as u8;
                    let elem = Elem::of(int_type(arr.size)).expect("an element");
                    let whole = Type::Vec(lanes * selem, elem);
                    // (memory's element k is register k % selem's lane k / selem)
                    if load {
                        let block = self.op(Op::Load { ty: whole, addr: base, order: None });
                        for r in 0..selem {
                            let mask = (0..lanes).map(|i| r + selem * i).collect();
                            let v = self.op(Op::Shuffle(block, block, mask));
                            self.vwrite(rt.plus(r), v);
                        }
                    } else {
                        let regs: Vec<Value> = (0..selem).map(|r| self.vread(rt.plus(r), ty)).collect();
                        let all = self.concat(&regs);
                        let mask = (0..lanes * selem).map(|k| (k % selem) * lanes + k / selem).collect();
                        let block = self.op(Op::Shuffle(all, all, mask));
                        self.op(Op::Store { addr: base, value: block, space: space(stack), order: None });
                    }
                }
            }
            Structure::Lane { size, index, selem } => {
                let ty = int_type(size);
                for r in 0..selem {
                    let a = at(self, u32::from(r) * size.bytes());
                    if load {
                        let v = self.op(Op::Load { ty, addr: a, order: None });
                        self.vwrite_lane(rt.plus(r), index, v);
                    } else {
                        let v = self.vlane(rt.plus(r), ty, index);
                        self.op(Op::Store { addr: a, value: v, space: space(stack), order: None });
                    }
                }
            }
            Structure::Replicate { arr, selem } => {
                let ty = int_type(arr.size);
                for r in 0..selem {
                    let a = at(self, u32::from(r) * arr.size.bytes());
                    let v = self.op(Op::Load { ty, addr: a, order: None });
                    let v = self.splat_to(vector_of(arr, ty), v);
                    self.vwrite(rt.plus(r), v);
                }
            }
        }
        if !load && !stack {
            self.op(Op::Mark { addr: base, bytes: what.bytes() });
        }
        if let Some(step) = step {
            let next = self.bin(BinOp::Add, base, step);
            self.write(rn, next, true);
        }
        Ok(())
    }

    /// Vectors of one type, one after another, as one vector (of their lanes together: 2 to 4 of them).
    pub(crate) fn concat(&mut self, parts: &[Value]) -> Value {
        let lanes = self.ty(parts[0]).lanes() as u8;
        let pair = |s: &mut Self, a: Value, b: Value, n: u8| s.op(Op::Shuffle(a, b, (0..2 * n).collect()));
        match parts {
            [a] => *a,
            [a, b] => pair(self, *a, *b, lanes),
            [a, b, c] => {
                let ab = pair(self, *a, *b, lanes);
                let cc = pair(self, *c, *c, lanes);
                let all = pair(self, ab, cc, 2 * lanes);
                self.op(Op::Shuffle(all, all, (0..3 * lanes).collect()))
            }
            [a, b, c, d] => {
                let ab = pair(self, *a, *b, lanes);
                let cd = pair(self, *c, *d, lanes);
                pair(self, ab, cd, 2 * lanes)
            }
            _ => unreachable!("1 to 4 vectors"),
        }
    }
}
