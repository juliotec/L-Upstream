//! The instructions' semantics: each decoded [`a64::Insn`] that is not a branch, as mid IR appended to a block.
//!
//! The semantics are the Arm ARM's (its pseudocode's AddWithCarry, DecodeBitMasks, ExtendReg, ShiftReg ...), written
//! from the architecture. Where the architecture leaves the choice to the system, they follow the host's model:
//! pointer authentication is the identity, the counter frequency, cache type and ZVA block are constants, a store
//! through SP marks no page, CAS marks its page whether it swapped or not.
//!
//! Branches, calls and supervisor calls are the function builder's (crate `lift`): it uses [`Sem::cond`] and the
//! register reads here for their operands.

use a64::{Cond, Reg};
use mir::{BinOp, BlockId, Cast, Flag, Function, Loc, Op, Pred, Type, UnOp, Value};

mod crypto;
mod fp;
mod fpfast;
mod fpscalar;
mod int;
mod mem;
mod simd;
mod simdmem;
mod sys;
mod vreg;

/// Why an instruction has no semantics here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fault {
    /// The lifter does not cover it yet (the text says which form): the guest stops at it, as at an undefined one.
    Unsupported(&'static str),
    /// The instruction is undefined here (an unknown system register, an EL1 operation): the guest stops at it.
    Undefined,
}

/// Lowers `insn`, the instruction at `pc`, into block `block` of `f`.
pub fn lower(f: &mut Function, block: BlockId, pc: u64, insn: &a64::Insn) -> Result<(), Fault> {
    Sem::new(f, block, pc).insn(insn)
}

/// The builder the semantics write with: a block of a function, and the address of the instruction being lowered.
pub struct Sem<'f> {
    f: &'f mut Function,
    b: BlockId,
    pc: u64,
}

impl<'f> Sem<'f> {
    /// A builder appending to `block` of `f`, for the instruction at `pc`.
    pub fn new(f: &'f mut Function, block: BlockId, pc: u64) -> Sem<'f> {
        Sem { f, b: block, pc }
    }

    fn insn(&mut self, insn: &a64::Insn) -> Result<(), Fault> {
        use a64::Insn::*;
        match *insn {
            Adr { .. }
            | AddSubImm { .. }
            | LogicalImm { .. }
            | MoveWide { .. }
            | Bitfield { .. }
            | Extr { .. }
            | LogicalReg { .. }
            | AddSubReg { .. }
            | AddSubExt { .. }
            | AddSubCarry { .. }
            | Rmif { .. }
            | Setf { .. }
            | CondCompare { .. }
            | CondSelect { .. }
            | DataProc1 { .. }
            | DataProc2 { .. }
            | Crc32 { .. }
            | Pac { .. }
            | Xpac { .. }
            | DataProc3 { .. } => self.data(insn),
            LoadStore { .. }
            | Pair { .. }
            | Exclusive { .. }
            | Ordered { .. }
            | Ldapr { .. }
            | Cas { .. }
            | Atomic { .. }
            | LdrPac { .. } => self.memory(insn),
            Hint { .. } | Clrex { .. } | Barrier { .. } | Pstate { .. } | Sys { .. } | Mrs { .. } | Msr { .. } => {
                self.system(insn)
            }
            Hvc { .. } | Smc { .. } | Hlt { .. } | Dcps { .. } => Err(Fault::Undefined),
            B { .. }
            | BCond { .. }
            | Cbz { .. }
            | Tbz { .. }
            | BranchReg { .. }
            | Svc { .. }
            | Brk { .. }
            | Udf { .. } => Err(Fault::Unsupported("a branch or a trap: the function builder's")),
            Fp1 { .. }
            | FpCvt { .. }
            | Fp2 { .. }
            | Fp3 { .. }
            | FpCmp { .. }
            | FpCondCmp { .. }
            | FpCsel { .. }
            | FpImm { .. }
            | FpToInt { .. }
            | IntToFp { .. }
            | FmovGen { .. } => self.scalar_fp(insn),
            Simd3 { .. }
            | SimdDiff { .. }
            | Simd2 { .. }
            | SimdAcross { .. }
            | SimdPairwise { .. }
            | SimdShift { .. }
            | SimdElem { .. }
            | SimdPermute { .. }
            | SimdExt { .. }
            | SimdTable { .. }
            | SimdDupElem { .. }
            | SimdDupGen { .. }
            | SimdInsElem { .. }
            | SimdInsGen { .. }
            | SimdMovToGen { .. }
            | SimdModImm { .. } => self.simd(insn),
            Aes { .. } | Sha3 { .. } | Sha2 { .. } => self.crypto(insn),
            SimdLdSt { .. } => self.structure(insn),
        }
    }

    // ---- building blocks

    /// Appends an op: its value.
    pub fn op(&mut self, op: Op) -> Value {
        self.f.push(self.b, op)
    }

    /// The constant `value` of `ty` (truncated to it).
    pub fn konst(&mut self, ty: Type, value: u128) -> Value {
        self.op(Op::Const { ty, value: value & ty.mask() })
    }

    /// An `i64` constant.
    pub fn k64(&mut self, value: u64) -> Value {
        self.konst(Type::I64, u128::from(value))
    }

    fn ty(&self, v: Value) -> Type {
        self.f.ty(v).expect("an operand with a value")
    }

    /// `a op b`.
    pub fn bin(&mut self, op: BinOp, a: Value, b: Value) -> Value {
        self.op(Op::Bin(op, a, b))
    }

    /// `op a`.
    pub fn un(&mut self, op: UnOp, a: Value) -> Value {
        self.op(Op::Un(op, a))
    }

    /// `a pred b`.
    pub fn cmp(&mut self, pred: Pred, a: Value, b: Value) -> Value {
        self.op(Op::Cmp(pred, a, b))
    }

    /// `c ? a : b`.
    pub fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        self.op(Op::Select(c, a, b))
    }

    /// `a` zero-extended, sign-extended or truncated to `ty` (nothing when it is already of it).
    pub fn resize(&mut self, a: Value, ty: Type, signed: bool) -> Value {
        let from = self.ty(a);
        match from.bits().cmp(&ty.bits()) {
            std::cmp::Ordering::Equal => a,
            std::cmp::Ordering::Less => self.op(Op::Cast(if signed { Cast::Sext } else { Cast::Zext }, ty, a)),
            std::cmp::Ordering::Greater => self.op(Op::Cast(Cast::Trunc, ty, a)),
        }
    }

    /// `a` zero-extended or truncated to `ty`.
    pub fn zext(&mut self, a: Value, ty: Type) -> Value {
        self.resize(a, ty, false)
    }

    /// `a` sign-extended or truncated to `ty`.
    pub fn sext(&mut self, a: Value, ty: Type) -> Value {
        self.resize(a, ty, true)
    }

    /// `a` shifted left by the constant `n` (nothing for 0).
    fn shl_k(&mut self, a: Value, n: u32) -> Value {
        if n == 0 {
            return a;
        }
        let k = self.k_like(a, u128::from(n));
        self.bin(BinOp::Shl, a, k)
    }

    /// `a` shifted right (logically) by the constant `n`.
    fn lshr_k(&mut self, a: Value, n: u32) -> Value {
        if n == 0 {
            return a;
        }
        let k = self.k_like(a, u128::from(n));
        self.bin(BinOp::LShr, a, k)
    }

    /// `a` shifted right (arithmetically) by the constant `n`.
    fn ashr_k(&mut self, a: Value, n: u32) -> Value {
        if n == 0 {
            return a;
        }
        let k = self.k_like(a, u128::from(n));
        self.bin(BinOp::AShr, a, k)
    }

    /// `a` rotated right by the constant `n` (modulo its width).
    fn ror_k(&mut self, a: Value, n: u32) -> Value {
        let w = self.ty(a).bits();
        let n = n % w;
        if n == 0 {
            return a;
        }
        let lo = self.lshr_k(a, n);
        let hi = self.shl_k(a, w - n);
        self.bin(BinOp::Or, lo, hi)
    }

    /// `a & k`.
    fn and_k(&mut self, a: Value, k: u128) -> Value {
        let k = self.k_like(a, k);
        self.bin(BinOp::And, a, k)
    }

    /// Bit `n` of `a`, an `i1`.
    fn bit(&mut self, a: Value, n: u32) -> Value {
        let shifted = self.lshr_k(a, n);
        self.zext(shifted, Type::I1)
    }

    /// Whether `a` is zero.
    fn is_zero(&mut self, a: Value) -> Value {
        let zero = self.k_like(a, 0);
        self.cmp(Pred::Eq, a, zero)
    }

    /// Whether `a` is negative (its top bit).
    fn is_negative(&mut self, a: Value) -> Value {
        let zero = self.k_like(a, 0);
        self.cmp(Pred::Slt, a, zero)
    }

    // ---- guest state

    /// A location's value.
    pub fn get(&mut self, loc: Loc) -> Value {
        self.op(Op::Get(loc))
    }

    /// A location's new value.
    pub fn set(&mut self, loc: Loc, v: Value) {
        self.op(Op::Set(loc, v));
    }

    /// A general register as an operand: X (`sf`) or W, register 31 the stack pointer when `sp`, else zero.
    pub fn read(&mut self, r: Reg, sf: bool, sp: bool) -> Value {
        let x = match (r.0, sp) {
            (31, true) => self.get(Loc::Sp),
            (31, false) => return self.konst(if sf { Type::I64 } else { Type::I32 }, 0),
            (n, _) => self.get(Loc::X(n)),
        };
        if sf { x } else { self.zext(x, Type::I32) }
    }

    /// An X register (register 31 is zero).
    pub fn x(&mut self, r: Reg) -> Value {
        self.read(r, true, false)
    }

    /// An X register or SP (register 31 is SP): a base address.
    pub fn xsp(&mut self, r: Reg) -> Value {
        self.read(r, true, true)
    }

    /// Writes a general register: a W value (`i32`) is zero-extended into the X register; register 31 is the stack
    /// pointer when `sp`, else the write is dropped.
    pub fn write(&mut self, r: Reg, v: Value, sp: bool) {
        let x = self.zext(v, Type::I64);
        match (r.0, sp) {
            (31, true) => self.set(Loc::Sp, x),
            (31, false) => {}
            (n, _) => self.set(Loc::X(n), x),
        }
    }

    /// A flag's value.
    pub fn flag(&mut self, flag: Flag) -> Value {
        self.get(Loc::Flag(flag))
    }

    /// Sets the four flags (each an `i1`).
    pub fn set_nzcv(&mut self, n: Value, z: Value, c: Value, v: Value) {
        for (flag, value) in [(Flag::N, n), (Flag::Z, z), (Flag::C, c), (Flag::V, v)] {
            self.set(Loc::Flag(flag), value);
        }
    }

    /// Whether condition `cond` holds now (ConditionHolds), an `i1`.
    pub fn cond(&mut self, cond: Cond) -> Value {
        let base = match cond.0 >> 1 {
            0 => self.flag(Flag::Z),
            1 => self.flag(Flag::C),
            2 => self.flag(Flag::N),
            3 => self.flag(Flag::V),
            4 => {
                let c = self.flag(Flag::C);
                let z = self.flag(Flag::Z);
                let nz = self.un(UnOp::Not, z);
                self.bin(BinOp::And, c, nz)
            }
            5 => {
                let n = self.flag(Flag::N);
                let v = self.flag(Flag::V);
                self.cmp(Pred::Eq, n, v)
            }
            6 => {
                let n = self.flag(Flag::N);
                let v = self.flag(Flag::V);
                let ge = self.cmp(Pred::Eq, n, v);
                let z = self.flag(Flag::Z);
                let nz = self.un(UnOp::Not, z);
                self.bin(BinOp::And, ge, nz)
            }
            _ => return self.konst(Type::I1, 1), // AL, NV
        };
        if cond.0 & 1 == 1 && cond.0 != 15 { self.un(UnOp::Not, base) } else { base }
    }

    /// The instruction's own address.
    pub fn pc(&self) -> u64 {
        self.pc
    }
}

#[cfg(test)]
mod tests;
