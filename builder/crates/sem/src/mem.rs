//! Loads, stores, exclusives and atomics.

use crate::{Fault, Sem};
use a64::{Addr, AtomicOp, Extend, Insn, LsKind, PairKind, Reg, Size, VReg};
use mir::{BinOp, Op, Order, Rmw, Space, Type, UnOp, Value};

/// An access's address: where it reads or writes, the base register's new value (pre- and post-index), and whether the
/// base is SP (a store there marks no page).
struct Address {
    at: Value,
    writeback: Option<(Reg, Value)>,
    stack: bool,
}

fn access_type(size: Size) -> Type {
    Type::of_bytes(size.bytes()).expect("an access of 1 to 16 bytes")
}

impl Sem<'_> {
    pub(crate) fn memory(&mut self, insn: &Insn) -> Result<(), Fault> {
        match *insn {
            Insn::LoadStore { kind, size, rt, addr, .. } => return self.single(kind, size, rt, addr),
            Insn::Pair { kind, size, rt, rt2, addr, .. } => return self.pair(kind, size, rt, rt2, addr),
            Insn::Exclusive { load, size, rs, rt, rt2, rn, .. } => {
                let at = self.xsp(rn);
                let bytes = size.bytes();
                match (load, rt2) {
                    (true, None) => {
                        let v = self.op(Op::Ldxr { addr: at, bytes });
                        self.write(rt, v, false);
                    }
                    (false, None) => {
                        let value = self.x(rt);
                        let status = self.op(Op::Stxr { addr: at, bytes, value });
                        self.write(rs, status, false);
                    }
                    (true, Some(rt2)) => {
                        let both = self.op(Op::Ldxp { addr: at, bytes });
                        let lo = self.zext(both, Type::I64);
                        let hi = self.lshr_k(both, 64);
                        let hi = self.zext(hi, Type::I64);
                        self.write(rt, lo, false);
                        self.write(rt2, hi, false);
                    }
                    (false, Some(rt2)) => {
                        let lo = self.x(rt);
                        let hi = self.x(rt2);
                        let status = self.op(Op::Stxp { addr: at, bytes, lo, hi });
                        self.write(rs, status, false);
                    }
                }
            }
            Insn::Ordered { load, size, rt, rn, .. } => {
                // (LDAR, LDLAR, STLR, STLLR: sequentially consistent; a release store marks, SP or not)
                let at = self.xsp(rn);
                let ty = access_type(size);
                if load {
                    let v = self.op(Op::Load { ty, addr: at, order: Some(Order::SeqCst) });
                    self.write(rt, v, false);
                } else {
                    let v = self.read(rt, size.0 == 3, false);
                    let v = self.zext(v, ty);
                    self.op(Op::Store { addr: at, value: v, space: Space::Guest, order: Some(Order::SeqCst) });
                    self.op(Op::Mark { addr: at, bytes: size.bytes() });
                }
            }
            Insn::Ldapr { size, rt, rn } => {
                let at = self.xsp(rn);
                let v = self.op(Op::Load { ty: access_type(size), addr: at, order: Some(Order::Acquire) });
                self.write(rt, v, false);
            }
            Insn::Cas { pair, size, rs, rt, rn, .. } => self.cas(pair, size, rs, rt, rn),
            Insn::Atomic { op, size, rs, rt, rn, .. } => {
                let at = self.xsp(rn);
                let ty = access_type(size);
                let v = self.read(rs, size.0 == 3, false);
                let v = self.zext(v, ty);
                let (rmw, v) = match op {
                    AtomicOp::Add => (Rmw::Add, v),
                    AtomicOp::Clr => (Rmw::And, self.un(UnOp::Not, v)),
                    AtomicOp::Eor => (Rmw::Xor, v),
                    AtomicOp::Set => (Rmw::Or, v),
                    AtomicOp::Smax => (Rmw::Max, v),
                    AtomicOp::Smin => (Rmw::Min, v),
                    AtomicOp::Umax => (Rmw::UMax, v),
                    AtomicOp::Umin => (Rmw::UMin, v),
                    AtomicOp::Swp => (Rmw::Xchg, v),
                };
                let old = self.op(Op::AtomicRmw { op: rmw, addr: at, value: v, order: Order::SeqCst });
                self.op(Op::Mark { addr: at, bytes: size.bytes() });
                self.write(rt, old, false);
            }
            Insn::LdrPac { writeback, rt, rn, imm, .. } => {
                // (authentication is the identity: the base plus the offset)
                let base = self.xsp(rn);
                let off = self.k64(imm as u64);
                let at = self.bin(BinOp::Add, base, off);
                let v = self.op(Op::Load { ty: Type::I64, addr: at, order: None });
                self.write(rt, v, false);
                if writeback {
                    self.write(rn, at, true);
                }
            }
            _ => unreachable!("not a memory access: {insn}"),
        }
        Ok(())
    }

    fn address(&mut self, addr: Addr, size: Size) -> Address {
        let offset = |s: &mut Self, rn: Reg, imm: i64| {
            let base = s.xsp(rn);
            let k = s.k64(imm as u64);
            (base, s.bin(BinOp::Add, base, k))
        };
        match addr {
            Addr::Offset { rn, imm } => {
                let (_, at) = offset(self, rn, imm);
                Address { at, writeback: None, stack: rn.is_31() }
            }
            Addr::PreIndex { rn, imm } => {
                let (_, at) = offset(self, rn, imm);
                Address { at, writeback: Some((rn, at)), stack: rn.is_31() }
            }
            Addr::PostIndex { rn, imm } => {
                let (base, next) = offset(self, rn, imm);
                Address { at: base, writeback: Some((rn, next)), stack: rn.is_31() }
            }
            Addr::RegOffset { rn, rm, extend, shifted } => {
                let base = self.xsp(rn);
                let amount = if shifted { u32::from(size.0) } else { 0 };
                let index = self.index(rm, extend, amount);
                let at = self.bin(BinOp::Add, base, index);
                Address { at, writeback: None, stack: rn.is_31() }
            }
            Addr::Literal { imm } => {
                let at = self.k64(self.pc().wrapping_add(imm as u64));
                Address { at, writeback: None, stack: false }
            }
        }
    }

    /// A register-offset access's index: Rm extended to 64 bits and shifted (ExtendReg).
    fn index(&mut self, rm: Reg, extend: Extend, amount: u32) -> Value {
        let x = self.x(rm);
        let v = if matches!(extend, Extend::Uxtw | Extend::Sxtw) { self.zext(x, Type::I32) } else { x };
        let v = self.resize(v, Type::I64, extend == Extend::Sxtw);
        self.shl_k(v, amount)
    }

    pub(crate) fn store(&mut self, at: Value, value: Value, bytes: u32, stack: bool) {
        self.op(Op::Store { addr: at, value, space: space(stack), order: None });
        if !stack {
            self.op(Op::Mark { addr: at, bytes });
        }
    }

    fn finish(&mut self, address: &Address) {
        if let Some((rn, v)) = address.writeback {
            self.write(rn, v, true);
        }
    }

    fn single(&mut self, kind: LsKind, size: Size, rt: Reg, addr: Addr) -> Result<(), Fault> {
        let ty = access_type(size);
        let a = self.address(addr, size);
        match kind {
            LsKind::Store => {
                let v = self.read(rt, size.0 == 3, false);
                let v = self.zext(v, ty);
                self.store(a.at, v, size.bytes(), a.stack);
            }
            LsKind::Load => {
                let v = self.op(Op::Load { ty, addr: a.at, order: None });
                self.write(rt, v, false);
            }
            LsKind::LoadSigned { x64 } => {
                let v = self.op(Op::Load { ty, addr: a.at, order: None });
                let v = self.sext(v, if x64 { Type::I64 } else { Type::I32 });
                self.write(rt, v, false);
            }
            LsKind::Prefetch => {} // (a hint)
            LsKind::StoreFp => {
                let v = self.vread(VReg(rt.0), ty);
                self.store(a.at, v, size.bytes(), a.stack);
            }
            LsKind::LoadFp => {
                let v = self.op(Op::Load { ty, addr: a.at, order: None });
                self.vwrite(VReg(rt.0), v);
            }
        }
        self.finish(&a);
        Ok(())
    }

    fn pair(&mut self, kind: PairKind, size: Size, rt: Reg, rt2: Reg, addr: Addr) -> Result<(), Fault> {
        let ty = access_type(size);
        let bytes = size.bytes();
        let a = self.address(addr, size);
        let step = self.k64(u64::from(bytes));
        let second = self.bin(BinOp::Add, a.at, step);
        match kind {
            PairKind::Store | PairKind::StoreFp => {
                let (v1, v2) = if kind == PairKind::Store {
                    (self.read(rt, size.0 == 3, false), self.read(rt2, size.0 == 3, false))
                } else {
                    (self.vread(VReg(rt.0), ty), self.vread(VReg(rt2.0), ty))
                };
                self.op(Op::Store { addr: a.at, value: v1, space: space(a.stack), order: None });
                self.op(Op::Store { addr: second, value: v2, space: space(a.stack), order: None });
                if !a.stack {
                    // (a pair marks once, for its whole span)
                    self.op(Op::Mark { addr: a.at, bytes: 2 * bytes });
                }
            }
            PairKind::Load | PairKind::LoadSignedWord => {
                let v1 = self.op(Op::Load { ty, addr: a.at, order: None });
                let v2 = self.op(Op::Load { ty, addr: second, order: None });
                let (v1, v2) = if kind == PairKind::LoadSignedWord {
                    (self.sext(v1, Type::I64), self.sext(v2, Type::I64))
                } else {
                    (v1, v2)
                };
                self.write(rt, v1, false);
                self.write(rt2, v2, false);
            }
            PairKind::LoadFp => {
                let v1 = self.op(Op::Load { ty, addr: a.at, order: None });
                let v2 = self.op(Op::Load { ty, addr: second, order: None });
                self.vwrite(VReg(rt.0), v1);
                self.vwrite(VReg(rt2.0), v2);
            }
        }
        self.finish(&a);
        Ok(())
    }

    /// CAS and CASP: sequentially consistent; the page marked whether it swapped or not.
    fn cas(&mut self, pair: bool, size: Size, rs: Reg, rt: Reg, rn: Reg) {
        let at = self.xsp(rn);
        let next = |r: Reg| Reg(r.0 + 1);
        if !pair {
            let ty = access_type(size);
            let expected = self.read(rs, size.0 == 3, false);
            let expected = self.zext(expected, ty);
            let new = self.read(rt, size.0 == 3, false);
            let new = self.zext(new, ty);
            let old = self.op(Op::CmpXchg { addr: at, expected, new });
            self.op(Op::Mark { addr: at, bytes: size.bytes() });
            self.write(rs, old, false);
            return;
        }
        // (CASP: a pair of `size`-byte halves, as one access twice as wide)
        let half = access_type(size);
        let whole = Type::int(half.bits() * 2).expect("a pair of words or doublewords");
        let join = |s: &mut Self, lo: Reg| {
            let a = s.read(lo, half == Type::I64, false);
            let b = s.read(next(lo), half == Type::I64, false);
            let a = s.zext(a, whole);
            let b = s.zext(b, whole);
            let b = s.shl_k(b, half.bits());
            s.bin(BinOp::Or, a, b)
        };
        let expected = join(self, rs);
        let new = join(self, rt);
        let old = self.op(Op::CmpXchg { addr: at, expected, new });
        self.op(Op::Mark { addr: at, bytes: 2 * size.bytes() });
        let lo = self.zext(old, half);
        let hi = self.lshr_k(old, half.bits());
        let hi = self.zext(hi, half);
        self.write(rs, lo, false);
        self.write(next(rs), hi, false);
    }
}

/// The space of a store through SP (`stack`: one the GPU never reads, so that it marks no page) or any other base.
pub(crate) fn space(stack: bool) -> Space {
    if stack { Space::Stack } else { Space::Guest }
}
