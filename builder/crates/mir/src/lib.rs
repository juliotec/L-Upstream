//! The lifter's mid IR: one guest function as blocks of typed SSA values.
//!
//! **The model.** Guest state (registers, flags, the system registers the game reads) lives in [`Loc`]ations, read with
//! [`Op::Get`] and written with [`Op::Set`]. Every value is defined once by one instruction, and is used only in the
//! block that defines it: what flows between blocks, and between guest instructions, is guest state. A writer gives each
//! location a local slot that the compiler keeps in a host register (LLVM's mem2reg) and moves it to and from the
//! context only where something outside the function can see it: calls, supervisor calls, hook sites, exits. So the IR
//! needs no phis, and a pass that edits it (a hook site, a fast path) cannot break SSA.
//!
//! **Memory.** Guest memory is one flat window ([`Op::Load`], [`Op::Store`]): an address is a 64-bit value. A store the
//! GPU may read marks its pages ([`Op::Mark`], explicit, so that a pass may merge them); a store through SP marks none.
//!
//! **Control.** A block ends in a [`Term`]: a jump or branch to a block of this function, a tail call of another
//! function, a return, an indirect jump, a switch over a recovered jump table, or a trap. An [`Edge`] that goes back
//! (to an address at or below the branch's) polls for an interrupt first, so that a loop never keeps its core.
//!
//! **Fast paths.** A computation that is cheap in the common case and long in the rare one (a float operation: the
//! host's IEEE op, unless a NaN or a flushed subnormal is involved) is an [`Op::Checked`]: its fast value, computed
//! inline, and a condition under which the value is instead a [`Helper`]'s, a pure function of its arguments written
//! once per module out of line. The writer branches around the helper's call, so the function's own IR stays short.

use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};

mod tables;
mod verify;
pub use verify::VerifyError;

/// A vector's element type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Elem {
    /// A lane mask's element (a comparison's).
    I1,
    /// A byte.
    I8,
    /// A halfword.
    I16,
    /// A word.
    I32,
    /// A doubleword.
    I64,
    /// A half-precision float.
    F16,
    /// A single-precision float.
    F32,
    /// A double-precision float.
    F64,
}

impl Elem {
    /// The scalar type of one element.
    pub fn scalar(self) -> Type {
        match self {
            Elem::I1 => Type::I1,
            Elem::I8 => Type::I8,
            Elem::I16 => Type::I16,
            Elem::I32 => Type::I32,
            Elem::I64 => Type::I64,
            Elem::F16 => Type::F16,
            Elem::F32 => Type::F32,
            Elem::F64 => Type::F64,
        }
    }

    /// The element of scalar type `ty` (no vector, no `i128`).
    pub fn of(ty: Type) -> Option<Elem> {
        Some(match ty {
            Type::I1 => Elem::I1,
            Type::I8 => Elem::I8,
            Type::I16 => Elem::I16,
            Type::I32 => Elem::I32,
            Type::I64 => Elem::I64,
            Type::F16 => Elem::F16,
            Type::F32 => Elem::F32,
            Type::F64 => Elem::F64,
            Type::I128 | Type::Vec(..) => return None,
        })
    }
}

/// A value's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// A condition or a flag.
    I1,
    /// A byte.
    I8,
    /// A halfword.
    I16,
    /// A word (a W register's value).
    I32,
    /// A doubleword (an X register's value, an address).
    I64,
    /// A quadword (a pair, a 64 × 64 product, a Q register's bits).
    I128,
    /// A half-precision float (IEEE binary16).
    F16,
    /// A single-precision float (binary32).
    F32,
    /// A double-precision float (binary64).
    F64,
    /// A vector of `.0` elements (2 to 16) of type `.1`.
    Vec(u8, Elem),
}

/// A SIMD and FP register's type: two doublewords (any other vector of 128 bits is a bitcast away).
pub const V128: Type = Type::Vec(2, Elem::I64);

/// The most setjmp return addresses an [`Op::SetjmpCall`] carries: the host's slot holds this many
/// (host/hos_setjmp.c `SjSlot.rets`). A setjmp owner with more is refused by the lift, not cut short.
pub const SETJMP_RETS: usize = 4;

impl Type {
    /// Its width in bits (a vector's: all its elements').
    pub fn bits(self) -> u32 {
        match self {
            Type::I1 => 1,
            Type::I8 => 8,
            Type::I16 | Type::F16 => 16,
            Type::I32 | Type::F32 => 32,
            Type::I64 | Type::F64 => 64,
            Type::I128 => 128,
            Type::Vec(n, e) => u32::from(n) * e.scalar().bits(),
        }
    }

    /// The integer type of `bits` bits (1, 8, 16, 32, 64, 128).
    pub fn int(bits: u32) -> Option<Type> {
        Some(match bits {
            1 => Type::I1,
            8 => Type::I8,
            16 => Type::I16,
            32 => Type::I32,
            64 => Type::I64,
            128 => Type::I128,
            _ => return None,
        })
    }

    /// The float type of `bits` bits (16, 32, 64).
    pub fn float(bits: u32) -> Option<Type> {
        Some(match bits {
            16 => Type::F16,
            32 => Type::F32,
            64 => Type::F64,
            _ => return None,
        })
    }

    /// The type of a `bytes`-byte memory access.
    pub fn of_bytes(bytes: u32) -> Option<Type> {
        Type::int(bytes * 8)
    }

    /// The mask of its bits (of an integer type).
    pub fn mask(self) -> u128 {
        if self.bits() >= 128 { u128::MAX } else { (1u128 << self.bits()) - 1 }
    }

    /// Its element type: a vector's elements', else itself.
    pub fn elem(self) -> Type {
        match self {
            Type::Vec(_, e) => e.scalar(),
            t => t,
        }
    }

    /// How many elements it has (1 for a scalar).
    pub fn lanes(self) -> u32 {
        match self {
            Type::Vec(n, _) => u32::from(n),
            _ => 1,
        }
    }

    /// The type of as many elements as this one has, each of `elem` (`elem` itself for a scalar).
    pub fn with_elem(self, elem: Type) -> Type {
        match self {
            Type::Vec(n, _) => Type::Vec(n, Elem::of(elem).expect("a scalar element")),
            _ => elem,
        }
    }

    /// Whether its elements are floats.
    pub fn is_float(self) -> bool {
        matches!(self.elem(), Type::F16 | Type::F32 | Type::F64)
    }

    /// Whether its elements are integers (or `i1`s).
    pub fn is_int(self) -> bool {
        !self.is_float()
    }

    /// Whether it is a vector.
    pub fn is_vector(self) -> bool {
        matches!(self, Type::Vec(..))
    }

    /// The integer type of the same shape: `f32` to `i32`, `<4 x float>` to `<4 x i32>`.
    pub fn as_int(self) -> Type {
        self.with_elem(Type::int(self.elem().bits()).expect("an element of 1 to 64 bits"))
    }
}

impl Display for Type {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Type::F16 => f.write_str("f16"),
            Type::F32 => f.write_str("f32"),
            Type::F64 => f.write_str("f64"),
            Type::Vec(n, e) => write!(f, "<{n} x {}>", e.scalar()),
            t => write!(f, "i{}", t.bits()),
        }
    }
}

/// A condition flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Flag {
    /// Negative.
    N,
    /// Zero.
    Z,
    /// Carry.
    C,
    /// Overflow.
    V,
}

/// A piece of guest state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Loc {
    /// X0 to X30 (an `i64`).
    X(u8),
    /// The stack pointer (an `i64`).
    Sp,
    /// A condition flag (an `i1`).
    Flag(Flag),
    /// TPIDR_EL0, the thread pointer (an `i64`).
    Tpidr,
    /// TPIDRRO_EL0, the OS's thread pointer (an `i64`).
    TpidrRo,
    /// FPCR (an `i64`: the guest's register alone).
    Fpcr,
    /// FPSR (an `i64`).
    Fpsr,
    /// V0 to V31, the SIMD and FP registers (a [`V128`]).
    V(u8),
}

impl Loc {
    /// The type of its value.
    pub fn ty(self) -> Type {
        match self {
            Loc::Flag(_) => Type::I1,
            Loc::V(_) => V128,
            _ => Type::I64,
        }
    }

    /// Its bit in a set of locations ([`Loc::all`]'s order: 72 bits of a `u128`).
    pub fn bit(self) -> u128 {
        let i = match self {
            Loc::X(n) => u32::from(n),
            Loc::Sp => 31,
            Loc::Flag(f) => 32 + f as u32,
            Loc::Tpidr => 36,
            Loc::TpidrRo => 37,
            Loc::Fpcr => 38,
            Loc::Fpsr => 39,
            Loc::V(n) => 40 + u32::from(n),
        };
        1 << i
    }

    /// The locations of a set of bits, in [`Loc::all`]'s order.
    pub fn from_bits(set: u128) -> Vec<Loc> {
        Loc::all().filter(|l| set & l.bit() != 0).collect()
    }

    /// Every location, in a fixed order (a writer's slots).
    pub fn all() -> impl Iterator<Item = Loc> {
        (0..31u8)
            .map(Loc::X)
            .chain([Loc::Sp])
            .chain([Flag::N, Flag::Z, Flag::C, Flag::V].map(Loc::Flag))
            .chain([Loc::Tpidr, Loc::TpidrRo, Loc::Fpcr, Loc::Fpsr])
            .chain((0..32u8).map(Loc::V))
    }
}

impl Display for Loc {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Loc::X(n) => write!(f, "x{n}"),
            Loc::Sp => f.write_str("sp"),
            Loc::Flag(flag) => write!(f, "{}", ["n", "z", "c", "v"][*flag as usize]),
            Loc::Tpidr => f.write_str("tpidr_el0"),
            Loc::TpidrRo => f.write_str("tpidrro_el0"),
            Loc::Fpcr => f.write_str("fpcr"),
            Loc::Fpsr => f.write_str("fpsr"),
            Loc::V(n) => write!(f, "v{n}"),
        }
    }
}

/// A two-operand integer operation, as LLVM defines it: a division by zero or a shift by the width or more is not
/// defined, so the semantics guard them (the guest's division by zero is 0; its shifts take the amount modulo the width).
/// Of vectors, each is lane by lane.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    UDiv,
    SDiv,
    And,
    Or,
    Xor,
    Shl,
    LShr,
    AShr,
    /// Saturating signed addition (LLVM's sadd.sat), and the rest.
    SAddSat,
    UAddSat,
    SSubSat,
    USubSat,
    /// The smaller of the two as signed numbers (LLVM's smin), and the rest.
    SMin,
    SMax,
    UMin,
    UMax,
}

/// A two-operand float operation, IEEE 754's, rounded to nearest (ties to even). A NaN result's bits are not defined:
/// the semantics choose them (the Arm rules) from the operands.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FBinOp {
    Add,
    Sub,
    Mul,
    Div,
    /// IEEE 754-2019's maximum (LLVM's `llvm.maximum`): a NaN if either is one, and −0 below +0.
    Maximum,
    /// IEEE 754-2019's minimum (LLVM's `llvm.minimum`): a NaN if either is one, and −0 below +0.
    Minimum,
}

/// A one-operand float operation (a NaN result's bits not defined, as [`FBinOp`]'s).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FUnOp {
    /// The square root, rounded to nearest.
    Sqrt,
    /// To an integral value, ties to even.
    RoundEven,
    /// To an integral value, ties away from zero.
    Round,
    /// Towards minus infinity.
    Floor,
    /// Towards plus infinity.
    Ceil,
    /// Towards zero.
    Trunc,
}

/// A float comparison's predicate: ordered (false when either is a NaN) or, `Uno`, whether either is.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FPred {
    Oeq,
    Ogt,
    Oge,
    Olt,
    Ole,
    Uno,
}

/// A class of floats [`Op::FClass`] tests for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FClass {
    /// Subnormal: not zero, its exponent field zero.
    Subnormal,
    /// A NaN, or subnormal.
    NanOrSubnormal,
}

/// A conversion of floats.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FCast {
    /// To a wider float (exact).
    Ext,
    /// To a narrower float, rounded to nearest.
    Trunc,
    /// A signed integer to a float, rounded to nearest.
    FromSigned,
    /// An unsigned integer to a float, rounded to nearest.
    FromUnsigned,
    /// A float to a signed integer: towards zero, saturating, a NaN to 0 (LLVM's fptosi.sat, Arm's FCVTZS).
    ToSigned,
    /// A float to an unsigned integer: towards zero, saturating, a NaN to 0.
    ToUnsigned,
}

/// An operation across the lanes of a vector, to a scalar.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reduce {
    Add,
    /// Whether any lane is set (of a lane mask, or any integer vector: the OR of its lanes).
    Or,
    SMax,
    SMin,
    UMax,
    UMin,
}

/// A constant table the IR may index (written once per module).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Table {
    /// The AES S-box (256 bytes).
    AesSbox,
    /// The AES inverse S-box (256 bytes).
    AesInvSbox,
    /// RecipSqrtEstimate's results less 256, for its operands 128 to 511 (the Arm ARM's FRSQRTE and URSQRTE): 384 bytes.
    RecipSqrt,
}

impl Table {
    /// How many entries it has.
    pub fn entries(self) -> u32 {
        match self {
            Table::AesSbox | Table::AesInvSbox => 256,
            Table::RecipSqrt => 384,
        }
    }

    /// Its bytes.
    pub fn bytes(self) -> Vec<u8> {
        match self {
            Table::AesSbox => tables::aes_sbox().to_vec(),
            Table::AesInvSbox => {
                let s = tables::aes_sbox();
                let mut inv = [0u8; 256];
                for (i, &v) in s.iter().enumerate() {
                    inv[usize::from(v)] = i as u8;
                }
                inv.to_vec()
            }
            Table::RecipSqrt => (128u32..512).map(|a| (tables::recip_sqrt_estimate(a) - 256) as u8).collect(),
        }
    }
}

/// A one-operand integer operation.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnOp {
    Not,
    Neg,
    /// Leading zeros (the width, for 0).
    Ctlz,
    /// The bits set.
    Ctpop,
    BitReverse,
    ByteSwap,
}

/// A comparison's predicate (signed or unsigned, as its name says).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pred {
    Eq,
    Ne,
    Ult,
    Ule,
    Ugt,
    Uge,
    Slt,
    Sle,
    Sgt,
    Sge,
}

/// A width change.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cast {
    Zext,
    Sext,
    Trunc,
}

/// An atomic access's ordering.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Order {
    Acquire,
    Release,
    SeqCst,
}

/// An atomic read-modify-write's operation.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rmw {
    Xchg,
    Add,
    And,
    Or,
    Xor,
    Max,
    Min,
    UMax,
    UMin,
}

/// Where a store goes, as far as the GPU's view of memory is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Space {
    /// Anywhere: the store marks its pages (an explicit [`Op::Mark`] follows it).
    Guest,
    /// Through SP: the guest stack, which the GPU never reads; no mark.
    Stack,
}

/// A value: the result of the instruction of that index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Value(pub u32);

impl Display for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// A block of the function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

impl Display for BlockId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.0)
    }
}

/// A [`Helper`] of the function: its index in [`Function::helpers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HelperId(pub u32);

/// An out-of-line pure function that [`Op::Checked`] calls on its slow path: written once per module by its name, so
/// the name must say everything its body computes (two helpers of one name are one helper).
///
/// Its body is one block of pure ops over its parameters ([`Op::Param`]), ending in [`Term::Return`]: what it returns
/// is `result`. It reads no guest state and has no effect; the verifier checks that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Helper {
    /// Its name in the module (without `@`).
    pub name: String,
    /// Its parameters' types.
    pub params: Vec<Type>,
    /// Its body.
    pub body: Function,
    /// The value it returns, of its body's block.
    pub result: Value,
}

impl Helper {
    /// The type it returns.
    pub fn ret(&self) -> Type {
        self.body.ty(self.result).expect("a helper's result is a value")
    }
}

/// A global of the host's that the IR reads or writes by its symbol: a hook site's flag byte or callback slot, a data
/// rule's value, the replaced call's callee (game/sites.h).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Global {
    /// Its C name.
    pub symbol: String,
    /// Whether it is `_Thread_local` (one per host thread).
    pub thread_local: bool,
}

impl Global {
    /// The process-wide global `symbol`.
    pub fn new(symbol: impl Into<String>) -> Global {
        Global { symbol: symbol.into(), thread_local: false }
    }

    /// The thread-local global `symbol`.
    pub fn thread_local(symbol: impl Into<String>) -> Global {
        Global { symbol: symbol.into(), thread_local: true }
    }
}

/// The host function an [`Op::HostCall`] calls.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Callee {
    /// The function of this C name.
    Named(String),
    /// The function whose address a global holds (a site's callback slot): read once, volatile, and called.
    Slot(Global),
}

impl Display for Callee {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Callee::Named(n) => f.write_str(n),
            Callee::Slot(g) => write!(f, "[{}]", g.symbol),
        }
    }
}

/// One argument of an [`Op::HostCall`].
///
/// The contract: a host call given [`HostArg::Ctx`] syncs the guest state (it goes to the context before the call and
/// comes back after); one without it syncs nothing, the state staying in the lifted function's slots, so its callee
/// must neither read nor write the guest's registers nor run guest code (host/game_call.c's game_call refuses a call
/// with no context, and a callee that needs to call the game is declared with the context).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HostArg {
    /// The guest context (`Ctx *`): the guest state goes to it before the call and comes back after.
    Ctx,
    /// A value of the function's (an integer or a float).
    Value(Value),
    /// The address of a function of this C name (a `void (*)(Ctx *)`, as `A64Fn`).
    Function(String),
}

/// A BLR's usual target ([`Op::CallIndirect`]): the address its register holds when it goes there, and the function
/// called directly for it, that address followed through PLT stubs (the two differ when the address is a stub's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Usual {
    /// The address the register is compared with.
    pub address: u64,
    /// The function called when it is that address.
    pub callee: u64,
}

/// An instruction.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// A constant (its low `ty.bits()` bits).
    Const {
        /// Its type.
        ty: Type,
        /// Its bits.
        value: u128,
    },
    /// The value of a location.
    Get(Loc),
    /// A location's new value (of the location's type).
    Set(Loc, Value),
    /// `a op b`, both of one type.
    Bin(BinOp, Value, Value),
    /// `op a`.
    Un(UnOp, Value),
    /// `a pred b`, an `i1`.
    Cmp(Pred, Value, Value),
    /// `cond ? a : b`; of vectors, `cond` is an `i1` for all their lanes or a lane mask (a `<n x i1>`).
    Select(Value, Value, Value),
    /// `a` widened or narrowed to a type (of vectors, each lane; their lane counts the same).
    Cast(Cast, Type, Value),
    /// `a`'s bits as a type of their width.
    Bitcast(Type, Value),
    /// A [`Helper`]'s parameter (only in a helper's body).
    Param {
        /// Which: 0 first.
        index: u8,
        /// Its type (the helper's `params[index]`).
        ty: Type,
    },
    /// `fast`, unless `slow` (an `i1`) holds: then the [`Helper`] `helper`'s value for `args`. The helper returns
    /// `fast`'s type; it is the exact computation, `fast` the one that agrees with it wherever `slow` does not hold.
    Checked {
        /// The fast path's value.
        fast: Value,
        /// When the helper's value is taken instead.
        slow: Value,
        /// The helper (one of [`Function::helpers`]).
        helper: HelperId,
        /// Its arguments, of its parameters' types.
        args: Vec<Value>,
    },
    /// Whether the float `a` is of the class: an `i1` (a lane mask of vectors).
    FClass(FClass, Value),
    /// `a op b`, floats of one type.
    FBin(FBinOp, Value, Value),
    /// `op a`, a float.
    FUn(FUnOp, Value),
    /// `a × b + c`, rounded once (to nearest).
    Fma(Value, Value, Value),
    /// `a pred b`, floats: an `i1` (a lane mask of vectors).
    FCmp(FPred, Value, Value),
    /// `a` converted to a type ([`FCast`]; of vectors, each lane).
    FCast(FCast, Type, Value),
    /// Element `.1` of the vector `.0`.
    Extract(Value, u8),
    /// The element of the vector `.0` that the integer `.1` (below its lane count) says.
    ExtractAt(Value, Value),
    /// The vector `.0` with element `.1` replaced by `.2`.
    Insert(Value, u8, Value),
    /// Elements of `a` (0 to n − 1) and `b` (n to 2n − 1) picked by the mask: a vector of the mask's length.
    Shuffle(Value, Value, Vec<u8>),
    /// A vector of a type with every element `a`.
    Splat(Type, Value),
    /// An operation across the lanes of an integer vector: its element type.
    Reduce(Reduce, Value),
    /// The byte of a [`Table`] at an index (an integer, below the table's length): an `i8`.
    Lookup(Table, Value),
    /// A load of `ty` from guest memory; atomic when `order` says.
    Load {
        /// What it reads.
        ty: Type,
        /// Its address (an `i64`).
        addr: Value,
        /// Its ordering, for an atomic load.
        order: Option<Order>,
    },
    /// A store to guest memory; atomic when `order` says.
    Store {
        /// Its address.
        addr: Value,
        /// What it writes.
        value: Value,
        /// Its space.
        space: Space,
        /// Its ordering, for an atomic store.
        order: Option<Order>,
    },
    /// The pages of `bytes` bytes from `addr` marked written (the first and the last byte's pages).
    Mark {
        /// The first byte.
        addr: Value,
        /// How many bytes.
        bytes: u32,
    },
    /// An atomic read-modify-write: the old value.
    AtomicRmw {
        /// The operation.
        op: Rmw,
        /// The address.
        addr: Value,
        /// The operand.
        value: Value,
        /// Its ordering.
        order: Order,
    },
    /// An atomic compare and swap, sequentially consistent: the old value.
    CmpXchg {
        /// The address.
        addr: Value,
        /// The value it must hold.
        expected: Value,
        /// The value it gets if it does.
        new: Value,
    },
    /// A full memory barrier.
    Fence,
    /// LDXR: an exclusive load of `bytes` (1 to 8) bytes, an `i64` (the runtime's monitor).
    Ldxr {
        /// The address.
        addr: Value,
        /// Its size.
        bytes: u32,
    },
    /// STXR: an exclusive store, its status (0 when it stored), an `i32`.
    Stxr {
        /// The address.
        addr: Value,
        /// Its size.
        bytes: u32,
        /// The value (an `i64`).
        value: Value,
    },
    /// LDXP: an exclusive load of two `bytes`-byte halves, an `i128` (the low half first).
    Ldxp {
        /// The address.
        addr: Value,
        /// Each half's size.
        bytes: u32,
    },
    /// STXP: an exclusive store of two halves, its status, an `i32`.
    Stxp {
        /// The address.
        addr: Value,
        /// Each half's size.
        bytes: u32,
        /// The low half (an `i64`).
        lo: Value,
        /// The high half (an `i64`).
        hi: Value,
    },
    /// CLREX.
    Clrex,
    /// The virtual counter (CNTVCT_EL0), an `i64`.
    Counter,
    /// A call of the guest function at `target` (whose X30 the semantics have set). The guest state is the callee's
    /// during it: a writer moves it to the context before and back after.
    Call {
        /// The callee's entry.
        target: u64,
    },
    /// A call of a function that calls `__setjmp` (a setjmp owner, whose X30 the semantics have set): the host takes a
    /// setjmp of its own around it (host/hos_setjmp.c), so that a guest longjmp to one of `rets`, the owner's setjmp
    /// return addresses (at most [`SETJMP_RETS`], the rest 0), comes back here and runs the owner's code from there. The
    /// whole guest state is reloaded after it.
    SetjmpCall {
        /// The owner's entry.
        target: u64,
        /// Its setjmp calls' return addresses.
        rets: [u64; SETJMP_RETS],
    },
    /// A call through a register (BLR), `usual`: the target it is called directly for (A64_DEVIRT), if any.
    CallIndirect {
        /// The target address (an `i64`).
        target: Value,
        /// A known usual target, called directly when it is the one.
        usual: Option<Usual>,
    },
    /// SVC #imm: the host's supervisor call, with the guest state in the context.
    Svc {
        /// The call's number.
        imm: u16,
    },
    /// A volatile load of a host global: an integer of `ty`.
    HostLoad {
        /// The global.
        global: Global,
        /// What it reads.
        ty: Type,
    },
    /// A volatile store of an integer to a host global.
    HostStore {
        /// The global.
        global: Global,
        /// What it writes.
        value: Value,
    },
    /// A call of a host function by the C ABI: its value of `ret`, or none. With [`HostArg::Ctx`] among its arguments
    /// the guest state goes to the context before it and comes back after, as across an SVC (the host may read and
    /// write any location); without, it reads and writes no guest state.
    HostCall {
        /// What it calls.
        callee: Callee,
        /// Its arguments, in order.
        args: Vec<HostArg>,
        /// What it returns, if anything (an integer or a float).
        ret: Option<Type>,
    },
}

/// Where an edge goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// A block of this function.
    Block(BlockId),
    /// Another function, by a tail call.
    Tail(u64),
    /// An address that is no entry and no instruction of this function: the guest stops there.
    BadTarget(u64),
    /// Another piece of the same split function ([`Pieces`]), entered at its entry block `entry` (an index into that
    /// piece's [`Function::entries`]): the state goes through the context, as across a tail call.
    Piece {
        /// The piece, 1 on (piece 0 is the function's own symbol, which goes to piece 1).
        piece: u32,
        /// Which of its entry blocks.
        entry: u32,
    },
}

/// What a function that is one piece of a split guest function ([`Function::pieces`]) is: its symbol is
/// `<base>.p<index>` (index 0, the guest function's own symbol, is `base` itself), and the pieces call each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pieces {
    /// The guest function's symbol (`f_<entry>`, or its body's).
    pub base: String,
    /// This piece's index.
    pub index: u32,
}

impl Pieces {
    /// The symbol of piece `index` of the function `base`.
    pub fn symbol(base: &str, index: u32) -> String {
        if index == 0 { base.to_string() } else { format!("{base}.p{index}") }
    }
}

/// An edge of the control-flow graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    /// Where it goes.
    pub to: Dest,
    /// Whether it polls for an interrupt first (a loop's back edge).
    pub poll: bool,
}

impl Edge {
    /// An edge to `to`, polling or not.
    pub fn new(to: Dest, poll: bool) -> Edge {
        Edge { to, poll }
    }
}

/// How a block ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// Always to one place.
    Jump(Edge),
    /// To `then` if `cond` (an `i1`), else to `otherwise`.
    Branch {
        /// The condition.
        cond: Value,
        /// Where it goes when true.
        then: Edge,
        /// Where it goes when false.
        otherwise: Edge,
    },
    /// A recovered jump table: to the case whose address `value` is; any other value is an indirect jump to it.
    Switch {
        /// The target address (an `i64`).
        value: Value,
        /// Each address the table holds, and where it goes.
        cases: Vec<(u64, Edge)>,
    },
    /// RET: back to the caller.
    Return,
    /// BR that is no recovered table: an indirect tail call of what the address holds.
    IndirectJump(Value),
    /// The guest stops at `pc` (BRK, UDF, an instruction no semantics cover), `word` its instruction.
    Trap {
        /// The instruction's address.
        pc: u64,
        /// Its word.
        word: u32,
    },
}

impl Op {
    /// The values it reads, for a pass to rewrite.
    pub fn operands_mut(&mut self) -> Vec<&mut Value> {
        match self {
            Op::Const { .. }
            | Op::Get(_)
            | Op::Param { .. }
            | Op::Fence
            | Op::Clrex
            | Op::Counter
            | Op::Call { .. }
            | Op::SetjmpCall { .. }
            | Op::Svc { .. }
            | Op::HostLoad { .. } => Vec::new(),
            Op::HostCall { args, .. } => args
                .iter_mut()
                .filter_map(|a| match a {
                    HostArg::Value(v) => Some(v),
                    HostArg::Ctx | HostArg::Function(_) => None,
                })
                .collect(),
            Op::HostStore { value: v, .. } => vec![v],
            Op::Set(_, v)
            | Op::Un(_, v)
            | Op::Cast(_, _, v)
            | Op::Bitcast(_, v)
            | Op::FUn(_, v)
            | Op::FCast(_, _, v)
            | Op::Extract(v, _)
            | Op::Splat(_, v)
            | Op::Reduce(_, v)
            | Op::Lookup(_, v)
            | Op::FClass(_, v) => vec![v],
            Op::Bin(_, a, b)
            | Op::Cmp(_, a, b)
            | Op::FBin(_, a, b)
            | Op::FCmp(_, a, b)
            | Op::ExtractAt(a, b)
            | Op::Insert(a, _, b)
            | Op::Shuffle(a, b, _) => vec![a, b],
            Op::Select(c, a, b) | Op::Fma(c, a, b) => vec![c, a, b],
            Op::Load { addr, .. } | Op::Mark { addr, .. } | Op::Ldxr { addr, .. } | Op::Ldxp { addr, .. } => vec![addr],
            Op::Store { addr, value, .. } | Op::AtomicRmw { addr, value, .. } | Op::Stxr { addr, value, .. } => {
                vec![addr, value]
            }
            Op::CmpXchg { addr, expected, new } => vec![addr, expected, new],
            Op::Stxp { addr, lo, hi, .. } => vec![addr, lo, hi],
            Op::CallIndirect { target, .. } => vec![target],
            Op::Checked { fast, slow, args, .. } => {
                let mut all = vec![fast, slow];
                all.extend(args.iter_mut());
                all
            }
        }
    }

    /// The values it reads.
    pub fn operands(&self) -> Vec<Value> {
        self.clone().operands_mut().into_iter().map(|v| *v).collect()
    }

    /// Whether it does nothing but make its value, so that a pass may drop it when nothing reads it: a pure op
    /// ([`Op::is_pure`]), a location's read, or a plain load (the guest reads memory it may read).
    pub fn is_removable(&self) -> bool {
        self.is_pure() || matches!(self, Op::Get(_) | Op::Load { order: None, .. })
    }

    /// Whether the guest state goes to the context and back across it (a call, an SVC): every location is read
    /// there, and may have changed after.
    pub fn exposes_state(&self) -> bool {
        match self {
            Op::Call { .. } | Op::SetjmpCall { .. } | Op::CallIndirect { .. } | Op::Svc { .. } => true,
            Op::HostCall { args, .. } => args.contains(&HostArg::Ctx),
            _ => false,
        }
    }

    /// Whether, across it, the calling convention's callee-saved registers (X19 to X29, SP) keep their values: a plain
    /// guest call's. A host call or an SVC may write any location, and a setjmp owner's call may come back by a
    /// longjmp, so every location is read again after those.
    pub fn keeps_callee_saved(&self) -> bool {
        matches!(self, Op::Call { .. } | Op::CallIndirect { .. })
    }
}

impl Term {
    /// The value it reads, if any, for a pass to rewrite.
    pub fn operand_mut(&mut self) -> Option<&mut Value> {
        match self {
            Term::Branch { cond: v, .. } | Term::Switch { value: v, .. } | Term::IndirectJump(v) => Some(v),
            Term::Jump(_) | Term::Return | Term::Trap { .. } => None,
        }
    }

    /// Whether the guest state leaves the function by it (to the caller, another function, the host): every location
    /// is read there. A jump to one of the function's blocks keeps it.
    pub fn exposes_state(&self) -> bool {
        match self {
            Term::Return | Term::IndirectJump(_) | Term::Trap { .. } | Term::Switch { .. } => true,
            Term::Jump(_) | Term::Branch { .. } => self.edges().iter().any(|e| !matches!(e.to, Dest::Block(_))),
        }
    }

    /// The edges it leaves by.
    pub fn edges(&self) -> Vec<Edge> {
        match self {
            Term::Jump(e) => vec![*e],
            Term::Branch { then, otherwise, .. } => vec![*then, *otherwise],
            Term::Switch { cases, .. } => cases.iter().map(|(_, e)| *e).collect(),
            Term::Return | Term::IndirectJump(_) | Term::Trap { .. } => Vec::new(),
        }
    }
}

/// One instruction: what it does and its result's type (`None`: it has no result).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inst {
    /// What it does.
    pub op: Op,
    /// Its result's type.
    pub ty: Option<Type>,
}

/// A block: the guest address it starts at, its instructions, and how it ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The guest address of its first instruction.
    pub pc: u64,
    /// Its instructions, in order.
    pub insts: Vec<Value>,
    /// How it ends (`None` while it is being built).
    pub term: Option<Term>,
    /// Whether control enters it as a guest block's start (at `pc`): a profile counts its entries. A block an edit
    /// made (a hook's call, the code after it) is not one.
    pub guest_start: bool,
    /// Whether its conditional branch, if it ends in one, is the guest's own (of the guest block at `pc`): a profile
    /// counts it taken. An edit's test (a hook's flag, its answer) is not.
    pub guest_term: bool,
}

/// What made a block, for [`Function::add_block_of`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockOrigin {
    /// A guest block's start.
    Guest,
    /// The rest of a guest block after an edit split it: its end is the guest block's.
    Rest,
    /// An edit's own code.
    Edit,
}

/// A guest function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// Its entry address: its name is `f_<entry>`.
    pub entry: u64,
    /// The block control enters at.
    pub start: BlockId,
    insts: Vec<Inst>,
    blocks: Vec<Block>,
    /// Each pure op already in a block, and its value: [`Function::push`] gives that value for the same op again.
    pure: HashMap<(BlockId, Op), Value>,
    /// The helpers its [`Op::Checked`]s call.
    helpers: Vec<Helper>,
    /// Its symbol when it is not `f_<entry>` (a function's body under a wrapper of that name).
    symbol: Option<String>,
    /// Whether it is a wrapper around a body of another symbol (no guest code of its own: a profile counts nothing).
    pub wrapper: bool,
    /// When it is a piece of a split function: which, of what. A piece other than 0 is internal to its module.
    pub pieces: Option<Pieces>,
    /// The blocks another piece may enter it at, by index ([`Dest::Piece`]'s `entry`); empty: its start alone.
    pub entries: Vec<BlockId>,
}

impl Function {
    /// An empty function of this entry, with its first block (at the entry).
    pub fn new(entry: u64) -> Function {
        let mut f = Function {
            entry,
            start: BlockId(0),
            insts: Vec::new(),
            blocks: Vec::new(),
            pure: HashMap::new(),
            helpers: Vec::new(),
            symbol: None,
            wrapper: false,
            pieces: None,
            entries: Vec::new(),
        };
        f.start = f.add_block(entry);
        f
    }

    /// Its name: `f_<entry>`, or the symbol it was given ([`Function::set_symbol`]).
    pub fn name(&self) -> String {
        self.symbol.clone().unwrap_or_else(|| format!("f_{:08x}", self.entry))
    }

    /// Its symbol when it is not `f_<entry>`.
    pub fn symbol(&self) -> Option<&str> {
        self.symbol.as_deref()
    }

    /// Names it `symbol` in place of `f_<entry>` (a body under a wrapper that has the guest function's name).
    pub fn set_symbol(&mut self, symbol: impl Into<String>) {
        self.symbol = Some(symbol.into());
    }

    /// A new, empty block starting at `pc` (a guest block's start).
    pub fn add_block(&mut self, pc: u64) -> BlockId {
        self.add_block_of(pc, BlockOrigin::Guest)
    }

    /// A new, empty block of the guest block at `pc`, made by `origin`.
    pub fn add_block_of(&mut self, pc: u64, origin: BlockOrigin) -> BlockId {
        let (guest_start, guest_term) = match origin {
            BlockOrigin::Guest => (true, true),
            BlockOrigin::Rest => (false, true),
            BlockOrigin::Edit => (false, false),
        };
        self.blocks.push(Block { pc, insts: Vec::new(), term: None, guest_start, guest_term });
        BlockId(self.blocks.len() as u32 - 1)
    }

    /// Says whether block `b` is a guest block's start ([`Block::guest_start`]).
    pub fn set_guest_start(&mut self, b: BlockId, guest: bool) {
        self.blocks[b.0 as usize].guest_start = guest;
    }

    /// How many instructions its blocks hold.
    pub fn inst_count(&self) -> usize {
        self.blocks.iter().map(|b| b.insts.len()).sum()
    }

    /// Says whether block `b`'s end is the guest's own ([`Block::guest_term`]).
    pub fn set_guest_term(&mut self, b: BlockId, guest: bool) {
        self.blocks[b.0 as usize].guest_term = guest;
    }

    /// The block `b`.
    pub fn block(&self, b: BlockId) -> &Block {
        &self.blocks[b.0 as usize]
    }

    /// Every block, with its id, in creation order.
    pub fn blocks(&self) -> impl Iterator<Item = (BlockId, &Block)> {
        self.blocks.iter().enumerate().map(|(i, b)| (BlockId(i as u32), b))
    }

    /// Each block's successors in the function, by index (its edges to its own blocks; a tail call, a piece's edge
    /// or a trap goes out of it).
    pub fn succs(&self) -> Vec<Vec<usize>> {
        self.blocks()
            .map(|(_, b)| {
                b.term
                    .as_ref()
                    .expect("a finished function")
                    .edges()
                    .iter()
                    .filter_map(|e| match e.to {
                        Dest::Block(t) => Some(t.0 as usize),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    }

    /// How many blocks it has.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// The instruction that defines `v`.
    pub fn inst(&self, v: Value) -> &Inst {
        &self.insts[v.0 as usize]
    }

    /// `v`'s type, if it is a value.
    pub fn ty(&self, v: Value) -> Option<Type> {
        self.insts.get(v.0 as usize).and_then(|i| i.ty)
    }

    /// Appends `op` to block `b`: its value. Its type is worked out from the op and its operands. A pure op (one whose
    /// value is a function of its operands: [`Op::is_pure`]) that block `b` already has is not appended again: its
    /// value is the earlier one's.
    pub fn push(&mut self, b: BlockId, op: Op) -> Value {
        if op.is_pure() {
            if let Some(&v) = self.pure.get(&(b, op.clone())) {
                return v;
            }
            let v = self.append(b, op.clone());
            self.pure.insert((b, op), v);
            return v;
        }
        self.append(b, op)
    }

    fn append(&mut self, b: BlockId, op: Op) -> Value {
        let ty = self.result_type(&op);
        let v = Value(self.insts.len() as u32);
        self.insts.push(Inst { op, ty });
        self.blocks[b.0 as usize].insts.push(v);
        v
    }

    /// A new instruction in no block yet, its type worked out as [`Function::push`]'s: a pass places it with
    /// [`Function::set_insts`].
    pub fn detached(&mut self, op: Op) -> Value {
        let ty = self.result_type(&op);
        let v = Value(self.insts.len() as u32);
        self.insts.push(Inst { op, ty });
        v
    }

    /// Block `b`'s instructions replaced by `insts`, in that order (a pass's rewrite of it; [`Function::verify`]
    /// checks that each is defined before its uses).
    pub fn set_insts(&mut self, b: BlockId, insts: Vec<Value>) {
        self.blocks[b.0 as usize].insts = insts;
    }

    /// The op that defines `v`, for a pass to rewrite (its type stays: a rewrite keeps the value's type).
    pub fn op_mut(&mut self, v: Value) -> &mut Op {
        &mut self.insts[v.0 as usize].op
    }

    /// Block `b`'s terminator, for a pass to rewrite.
    pub fn term_mut(&mut self, b: BlockId) -> Option<&mut Term> {
        self.blocks[b.0 as usize].term.as_mut()
    }

    /// Ends block `b` with `term`.
    pub fn terminate(&mut self, b: BlockId, term: Term) {
        self.blocks[b.0 as usize].term = Some(term);
    }

    /// Whether block `b` has its terminator.
    pub fn is_terminated(&self, b: BlockId) -> bool {
        self.blocks[b.0 as usize].term.is_some()
    }

    /// Its helpers, with their ids.
    pub fn helpers(&self) -> impl Iterator<Item = (HelperId, &Helper)> {
        self.helpers.iter().enumerate().map(|(i, h)| (HelperId(i as u32), h))
    }

    /// The helper `id`.
    pub fn helper(&self, id: HelperId) -> &Helper {
        &self.helpers[id.0 as usize]
    }

    /// The helper of this name, if the function has it.
    pub fn helper_named(&self, name: &str) -> Option<HelperId> {
        self.helpers.iter().position(|h| h.name == name).map(|i| HelperId(i as u32))
    }

    /// `h` added to its helpers (or the one of its name it has already): its id.
    pub fn add_helper(&mut self, h: Helper) -> HelperId {
        if let Some(id) = self.helper_named(&h.name) {
            debug_assert!(self.helper(id) == &h, "two helpers named {}", h.name);
            return id;
        }
        self.helpers.push(h);
        HelperId(self.helpers.len() as u32 - 1)
    }

    /// The locations any instruction of the function sets.
    pub fn written(&self) -> std::collections::BTreeSet<Loc> {
        self.insts
            .iter()
            .filter_map(|i| match i.op {
                Op::Set(loc, _) => Some(loc),
                _ => None,
            })
            .collect()
    }

    fn result_type(&self, op: &Op) -> Option<Type> {
        let of = |v: &Value| self.ty(*v);
        match op {
            Op::Const { ty, .. }
            | Op::Load { ty, .. }
            | Op::Cast(_, ty, _)
            | Op::Bitcast(ty, _)
            | Op::FCast(_, ty, _)
            | Op::Param { ty, .. }
            | Op::Splat(ty, _) => Some(*ty),
            Op::Get(loc) => Some(loc.ty()),
            Op::Bin(_, a, _) | Op::Un(_, a) | Op::FBin(_, a, _) | Op::FUn(_, a) | Op::Fma(a, ..) => of(a),
            Op::Select(_, a, _) | Op::Insert(a, ..) => of(a),
            Op::FCmp(_, a, _) | Op::FClass(_, a) => of(a).map(|t| t.with_elem(Type::I1)),
            Op::Checked { fast, .. } => of(fast),
            Op::Extract(a, _) | Op::ExtractAt(a, _) | Op::Reduce(_, a) => of(a).map(Type::elem),
            Op::Shuffle(a, _, mask) => {
                of(a).and_then(|t| Some(Type::Vec(u8::try_from(mask.len()).ok()?, Elem::of(t.elem())?)))
            }
            Op::Lookup(..) => Some(Type::I8),
            Op::AtomicRmw { value, .. } => of(value),
            Op::CmpXchg { expected, .. } => of(expected),
            Op::Cmp(_, a, _) => of(a).map(|t| t.with_elem(Type::I1)),
            Op::Ldxr { .. } | Op::Counter => Some(Type::I64),
            Op::HostLoad { ty, .. } => Some(*ty),
            Op::HostCall { ret, .. } => *ret,
            Op::Ldxp { .. } => Some(Type::I128),
            Op::Stxr { .. } | Op::Stxp { .. } => Some(Type::I32),
            Op::Set(..)
            | Op::Store { .. }
            | Op::Mark { .. }
            | Op::Fence
            | Op::Clrex
            | Op::Call { .. }
            | Op::SetjmpCall { .. }
            | Op::CallIndirect { .. }
            | Op::Svc { .. }
            | Op::HostStore { .. } => None,
        }
    }

    /// Checks the IR's rules: every block ends; every operand is a value of this function, defined earlier in the same
    /// block, of the type its op needs; every edge goes to a block that exists; every fast path's helper is one of
    /// the function's, called with its parameters' types, and every helper is one block of pure ops over its
    /// parameters that returns a value.
    pub fn verify(&self) -> Result<(), VerifyError> {
        verify::verify(self)
    }
}

/// Checks the pieces of one split function together (`pieces[i]` is piece `i`, each verified on its own): every edge
/// from one to another names a piece there is and one of its entry blocks.
pub fn verify_pieces(pieces: &[Function]) -> Result<(), VerifyError> {
    verify::verify_pieces(pieces)
}

impl Op {
    /// Whether its value is a function of its operands alone: it reads no state and has no effect (constants, the
    /// arithmetic, the casts, the lane operations, the table lookups).
    pub fn is_pure(&self) -> bool {
        match self {
            Op::Const { .. }
            | Op::Bin(..)
            | Op::Un(..)
            | Op::Cmp(..)
            | Op::Select(..)
            | Op::Cast(..)
            | Op::Bitcast(..)
            | Op::FBin(..)
            | Op::FUn(..)
            | Op::Fma(..)
            | Op::FCmp(..)
            | Op::FCast(..)
            | Op::Extract(..)
            | Op::ExtractAt(..)
            | Op::Insert(..)
            | Op::Shuffle(..)
            | Op::Splat(..)
            | Op::Reduce(..)
            | Op::Lookup(..)
            | Op::Param { .. }
            | Op::Checked { .. }
            | Op::FClass(..) => true,
            Op::Get(_)
            | Op::Set(..)
            | Op::Load { .. }
            | Op::Store { .. }
            | Op::Mark { .. }
            | Op::AtomicRmw { .. }
            | Op::CmpXchg { .. }
            | Op::Fence
            | Op::Ldxr { .. }
            | Op::Stxr { .. }
            | Op::Ldxp { .. }
            | Op::Stxp { .. }
            | Op::Clrex
            | Op::Counter
            | Op::Call { .. }
            | Op::SetjmpCall { .. }
            | Op::CallIndirect { .. }
            | Op::Svc { .. }
            | Op::HostLoad { .. }
            | Op::HostStore { .. }
            | Op::HostCall { .. } => false,
        }
    }
}

impl Display for Op {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Op::Const { ty, value } => write!(f, "const {ty} {value:#x}"),
            Op::Get(loc) => write!(f, "get {loc}"),
            Op::Set(loc, v) => write!(f, "set {loc}, {v}"),
            Op::Bin(op, a, b) => write!(f, "{} {a}, {b}", format!("{op:?}").to_lowercase()),
            Op::Un(op, a) => write!(f, "{} {a}", format!("{op:?}").to_lowercase()),
            Op::Cmp(p, a, b) => write!(f, "cmp {} {a}, {b}", format!("{p:?}").to_lowercase()),
            Op::Select(c, a, b) => write!(f, "select {c}, {a}, {b}"),
            Op::Cast(c, ty, a) => write!(f, "{} {a} to {ty}", format!("{c:?}").to_lowercase()),
            Op::Bitcast(ty, a) => write!(f, "bitcast {a} to {ty}"),
            Op::Param { index, .. } => write!(f, "param {index}"),
            Op::Checked { fast, slow, helper, args } => {
                let args: Vec<String> = args.iter().map(Value::to_string).collect();
                write!(f, "{fast} unless {slow}: h{}({})", helper.0, args.join(", "))
            }
            Op::FClass(c, a) => write!(f, "fclass {} {a}", format!("{c:?}").to_lowercase()),
            Op::FBin(op, a, b) => write!(f, "f{} {a}, {b}", format!("{op:?}").to_lowercase()),
            Op::FUn(op, a) => write!(f, "f{} {a}", format!("{op:?}").to_lowercase()),
            Op::Fma(a, b, c) => write!(f, "fma {a}, {b}, {c}"),
            Op::FCmp(p, a, b) => write!(f, "fcmp {} {a}, {b}", format!("{p:?}").to_lowercase()),
            Op::FCast(c, ty, a) => write!(f, "f{} {a} to {ty}", format!("{c:?}").to_lowercase()),
            Op::Extract(a, i) => write!(f, "extract {a}[{i}]"),
            Op::ExtractAt(a, i) => write!(f, "extract {a}[{i}]"),
            Op::Insert(a, i, x) => write!(f, "insert {a}[{i}], {x}"),
            Op::Shuffle(a, b, m) => write!(f, "shuffle {a}, {b}, {m:?}"),
            Op::Splat(ty, a) => write!(f, "splat {a} to {ty}"),
            Op::Reduce(op, a) => write!(f, "reduce {} {a}", format!("{op:?}").to_lowercase()),
            Op::Lookup(t, a) => write!(f, "lookup {t:?}[{a}]"),
            Op::Load { ty, addr, order } => write!(f, "load {ty} [{addr}]{}", order_text(*order)),
            Op::Store { addr, value, space, order } => {
                write!(
                    f,
                    "store [{addr}], {value}{}{}",
                    if *space == Space::Stack { " stack" } else { "" },
                    order_text(*order)
                )
            }
            Op::Mark { addr, bytes } => write!(f, "mark [{addr}], {bytes}"),
            Op::AtomicRmw { op, addr, value, order } => {
                write!(f, "atomic {} [{addr}], {value} {order:?}", format!("{op:?}").to_lowercase())
            }
            Op::CmpXchg { addr, expected, new } => write!(f, "cmpxchg [{addr}], {expected}, {new}"),
            Op::Fence => f.write_str("fence"),
            Op::Ldxr { addr, bytes } => write!(f, "ldxr [{addr}], {bytes}"),
            Op::Stxr { addr, bytes, value } => write!(f, "stxr [{addr}], {bytes}, {value}"),
            Op::Ldxp { addr, bytes } => write!(f, "ldxp [{addr}], {bytes}"),
            Op::Stxp { addr, bytes, lo, hi } => write!(f, "stxp [{addr}], {bytes}, {lo}, {hi}"),
            Op::Clrex => f.write_str("clrex"),
            Op::Counter => f.write_str("counter"),
            Op::Call { target } => write!(f, "call f_{target:08x}"),
            Op::SetjmpCall { target, rets } => {
                write!(
                    f,
                    "call f_{target:08x} with setjmp ({:x}, {:x}, {:x}, {:x})",
                    rets[0], rets[1], rets[2], rets[3]
                )
            }
            Op::CallIndirect { target, usual: None } => write!(f, "call [{target}]"),
            Op::CallIndirect { target, usual: Some(u) } if u.address == u.callee => {
                write!(f, "call [{target}] usually f_{:08x}", u.callee)
            }
            Op::CallIndirect { target, usual: Some(u) } => {
                write!(f, "call [{target}] usually {:#x}: f_{:08x}", u.address, u.callee)
            }
            Op::Svc { imm } => write!(f, "svc {imm:#x}"),
            Op::HostLoad { global, ty } => write!(f, "host load {ty} @{}", global.symbol),
            Op::HostStore { global, value } => write!(f, "host store @{}, {value}", global.symbol),
            Op::HostCall { callee, args, .. } => {
                let args: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        HostArg::Ctx => "ctx".to_string(),
                        HostArg::Value(v) => v.to_string(),
                        HostArg::Function(n) => format!("@{n}"),
                    })
                    .collect();
                write!(f, "host call {callee}({})", args.join(", "))
            }
        }
    }
}

fn order_text(order: Option<Order>) -> String {
    order.map_or_else(String::new, |o| format!(" {o:?}"))
}

impl Display for Edge {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.to {
            Dest::Block(b) => write!(f, "{b}")?,
            Dest::Tail(t) => write!(f, "tail f_{t:08x}")?,
            Dest::BadTarget(t) => write!(f, "bad {t:#x}")?,
            Dest::Piece { piece, entry } => write!(f, "piece {piece} entry {entry}")?,
        }
        if self.poll { f.write_str(" (poll)") } else { Ok(()) }
    }
}

impl Display for Term {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Term::Jump(e) => write!(f, "jump {e}"),
            Term::Branch { cond, then, otherwise } => write!(f, "branch {cond}, {then}, {otherwise}"),
            Term::Switch { value, cases } => {
                write!(f, "switch {value}")?;
                for (a, e) in cases {
                    write!(f, ", {a:#x}: {e}")?;
                }
                f.write_str(", else jump [value]")
            }
            Term::Return => f.write_str("return"),
            Term::IndirectJump(v) => write!(f, "jump [{v}]"),
            Term::Trap { pc, word } => write!(f, "trap {pc:#x} {word:08x}"),
        }
    }
}

impl Display for Function {
    /// A readable dump: each block, its instructions, its end; then its helpers.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} (start {}):", self.name(), self.start)?;
        self.fmt_blocks(f)?;
        for (id, h) in self.helpers() {
            let params: Vec<String> = h.params.iter().map(Type::to_string).collect();
            writeln!(f, "h{} {} ({}) -> {}:", id.0, h.name, params.join(", "), h.result)?;
            h.body.fmt_blocks(f)?;
        }
        Ok(())
    }
}

impl Function {
    fn fmt_blocks(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (id, b) in self.blocks() {
            writeln!(f, "{id} @{:x}:", b.pc)?;
            for &v in &b.insts {
                let inst = self.inst(v);
                match inst.ty {
                    Some(ty) => writeln!(f, "  {v}: {ty} = {}", inst.op)?,
                    None => writeln!(f, "  {}", inst.op)?,
                }
            }
            match &b.term {
                Some(t) => writeln!(f, "  {t}")?,
                None => writeln!(f, "  (no end)")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_prints() {
        let mut f = Function::new(0x8000_1000);
        let b = f.start;
        let x1 = f.push(b, Op::Get(Loc::X(1)));
        let four = f.push(b, Op::Const { ty: Type::I64, value: 4 });
        let sum = f.push(b, Op::Bin(BinOp::Add, x1, four));
        f.push(b, Op::Set(Loc::X(0), sum));
        f.terminate(b, Term::Return);
        f.verify().unwrap();
        let text = f.to_string();
        assert!(text.contains("v2: i64 = add v0, v1"), "{text}");
        assert!(text.contains("set x0, v2"));
        assert_eq!(f.written().into_iter().collect::<Vec<_>>(), vec![Loc::X(0)]);
    }
}
