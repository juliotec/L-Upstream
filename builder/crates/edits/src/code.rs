//! A small statement language for the code an edit adds to a function, and its translation into mid IR.
//!
//! An edit is Rust data, never C text: [`Stmt`]s over [`Expr`]essions of the guest state, guest memory, the host's
//! globals and the host's functions. [`emit`] writes them into a function being built, through [`Emit`], which the
//! function builder implements: it says where a `goto` goes ([`Target`]) and lowers the guest instruction an edit
//! keeps ([`Stmt::Insn`], a replaced instruction's own code while its site is off).
//!
//! The IR's rule that a value is used only in its own block holds here as well: an expression is evaluated in the
//! block where its statement is, and a [`Stmt::Let`] value ([`Expr::Var`]) lives until the next statement that
//! branches. Conditions ([`Cond`]) short-circuit as C's `&&` and `||` do: the right side runs only when it decides.

use mir::{
    BinOp, BlockId, BlockOrigin, Callee, Cast, Dest, Edge, FBinOp, FCast, Global, HostArg, Loc, Op, Pred, Space, Term,
    Type, Value,
};
use std::collections::HashMap;

/// An expression: a value computed in one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// A location's value (an `i64`, an `i1` flag, a V register's `<2 x i64>`).
    Reg(Loc),
    /// A constant integer of a type.
    Const(Type, u128),
    /// A load of a type from guest memory.
    Load(Type, Box<Expr>),
    /// A host global's value, read once (volatile) as an integer of a type.
    Global(Global, Type),
    /// A host function's value: its callee, its arguments, and what it returns.
    Call(Callee, Vec<Arg>, Type),
    /// An integer operation of two values of one type.
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// An integer comparison: an `i1`.
    Cmp(Pred, Box<Expr>, Box<Expr>),
    /// `cond ? a : b`, `cond` an `i1`.
    Select(Box<Expr>, Box<Expr>, Box<Expr>),
    /// A width change.
    Cast(Cast, Type, Box<Expr>),
    /// The same bits as another type.
    Bitcast(Type, Box<Expr>),
    /// A float conversion.
    FCast(FCast, Type, Box<Expr>),
    /// A float operation (the host's IEEE arithmetic, as C's).
    FBin(FBinOp, Box<Expr>, Box<Expr>),
    /// An element of a vector.
    Extract(Box<Expr>, u8),
    /// The value a [`Stmt::Let`] of this number gave, in the same block.
    Var(u8),
}

/// An argument of a host function's call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    /// The guest context: the guest state is in it during the call.
    Ctx,
    /// A value.
    Value(Expr),
    /// The address of the function of this symbol.
    Function(String),
}

/// A condition, short-circuiting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cond {
    /// Two integers compared.
    Test(Pred, Expr, Expr),
    /// Both, the second evaluated only when the first holds.
    And(Box<Cond>, Box<Cond>),
    /// Either, the second evaluated only when the first does not hold.
    Or(Box<Cond>, Box<Cond>),
}

/// Where a `goto` goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The instruction at this address, in the same function.
    Pc(u64),
    /// Where the conditional branch the code is at goes when taken (a `branch` site's answer).
    Taken,
}

/// A statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stmt {
    /// A location's new value.
    Set(Loc, Expr),
    /// A store to guest memory of the value's width (its pages marked written, as every guest store's).
    Store(Expr, Expr),
    /// A volatile store to a host global.
    SetGlobal(Global, Expr),
    /// A host function called for what it does (its value, if any, dropped).
    Call(Callee, Vec<Arg>, Option<Type>),
    /// A value named for the statements after it, in the same block ([`Expr::Var`]).
    Let(u8, Expr),
    /// The first list when the condition holds, else the second.
    If(Cond, Vec<Stmt>, Vec<Stmt>),
    /// Return from the function at once.
    Return,
    /// Go to a target.
    Goto(Target),
    /// The guest instruction the code is at, as it is (a replaced instruction's own code).
    Insn,
}

/// What [`emit`] needs of the function builder.
pub trait Emit {
    /// What lowering the guest instruction may fail with.
    type Error;
    /// The function being built.
    fn func(&mut self) -> &mut mir::Function;
    /// The start of the guest block the code is in (the blocks the code makes are of it).
    fn guest_block(&self) -> u64;
    /// The edge for a `goto` to `to` from the code's instruction.
    fn edge(&mut self, to: Target) -> Edge;
    /// The guest instruction the code is at, lowered into block `b` (which it may end: a trap).
    fn insn(&mut self, b: BlockId) -> Result<(), Self::Error>;
}

/// `code` written into block `b`: the block where control goes on after it, or `None` when every way through it
/// ends (a return, a goto).
pub fn emit<E: Emit>(e: &mut E, b: BlockId, code: &[Stmt]) -> Result<Option<BlockId>, E::Error> {
    Writer { e, vars: HashMap::new() }.stmts(b, code)
}

/// A condition written at the end of block `b`: to `then` when it holds, else to `otherwise`.
pub fn emit_cond<E: Emit>(e: &mut E, b: BlockId, c: &Cond, then: Edge, otherwise: Edge) {
    Writer { e, vars: HashMap::new() }.cond(b, c, then, otherwise);
}

struct Writer<'e, E: Emit> {
    e: &'e mut E,
    /// The [`Stmt::Let`] values of the block being written, and that block.
    vars: HashMap<u8, (BlockId, Value)>,
}

fn to_block(b: BlockId) -> Edge {
    Edge::new(Dest::Block(b), false)
}

impl<E: Emit> Writer<'_, E> {
    fn block(&mut self, origin: BlockOrigin) -> BlockId {
        let pc = self.e.guest_block();
        self.e.func().add_block_of(pc, origin)
    }

    fn stmts(&mut self, mut b: BlockId, code: &[Stmt]) -> Result<Option<BlockId>, E::Error> {
        for (i, s) in code.iter().enumerate() {
            match self.stmt(b, s)? {
                Some(next) => b = next,
                None => {
                    debug_assert!(i + 1 == code.len(), "code after a return or a goto: {:?}", &code[i + 1..]);
                    return Ok(None);
                }
            }
        }
        Ok(Some(b))
    }

    fn stmt(&mut self, b: BlockId, s: &Stmt) -> Result<Option<BlockId>, E::Error> {
        match s {
            Stmt::Set(loc, x) => {
                let v = self.expr(b, x);
                self.e.func().push(b, Op::Set(*loc, v));
            }
            Stmt::Store(addr, x) => {
                let a = self.expr(b, addr);
                let v = self.expr(b, x);
                let f = self.e.func();
                let bytes = f.ty(v).expect("a value").bits() / 8;
                f.push(b, Op::Store { addr: a, value: v, space: Space::Guest, order: None });
                f.push(b, Op::Mark { addr: a, bytes });
            }
            Stmt::SetGlobal(g, x) => {
                let v = self.expr(b, x);
                self.e.func().push(b, Op::HostStore { global: g.clone(), value: v });
            }
            Stmt::Call(callee, args, ret) => {
                let args = self.args(b, args);
                self.e.func().push(b, Op::HostCall { callee: callee.clone(), args, ret: *ret });
            }
            Stmt::Let(n, x) => {
                let v = self.expr(b, x);
                self.vars.insert(*n, (b, v));
            }
            Stmt::If(c, then, otherwise) => {
                let (tb, eb, join) =
                    (self.block(BlockOrigin::Edit), self.block(BlockOrigin::Edit), self.block(BlockOrigin::Rest));
                self.cond(b, c, to_block(tb), to_block(eb));
                for (from, code) in [(tb, then), (eb, otherwise)] {
                    if let Some(end) = self.stmts(from, code)? {
                        self.e.func().terminate(end, Term::Jump(to_block(join)));
                    }
                }
                return Ok(Some(join));
            }
            Stmt::Return => {
                self.e.func().terminate(b, Term::Return);
                return Ok(None);
            }
            Stmt::Goto(t) => {
                let edge = self.e.edge(*t);
                self.e.func().terminate(b, Term::Jump(edge));
                return Ok(None);
            }
            Stmt::Insn => {
                self.e.insn(b)?;
                if self.e.func().is_terminated(b) {
                    return Ok(None);
                }
            }
        }
        Ok(Some(b))
    }

    /// Ends `b` with the condition's test (its own, an edit's: no profile counts it), on to `then` or `otherwise`.
    fn cond(&mut self, b: BlockId, c: &Cond, then: Edge, otherwise: Edge) {
        match c {
            Cond::Test(p, x, y) => {
                let (x, y) = (self.expr(b, x), self.expr(b, y));
                let f = self.e.func();
                let cond = f.push(b, Op::Cmp(*p, x, y));
                f.terminate(b, Term::Branch { cond, then, otherwise });
                f.set_guest_term(b, false);
            }
            Cond::And(x, y) => {
                let mid = self.block(BlockOrigin::Edit);
                self.cond(b, x, to_block(mid), otherwise);
                self.cond(mid, y, then, otherwise);
            }
            Cond::Or(x, y) => {
                let mid = self.block(BlockOrigin::Edit);
                self.cond(b, x, then, to_block(mid));
                self.cond(mid, y, then, otherwise);
            }
        }
    }

    fn args(&mut self, b: BlockId, args: &[Arg]) -> Vec<HostArg> {
        args.iter()
            .map(|a| match a {
                Arg::Ctx => HostArg::Ctx,
                Arg::Value(x) => HostArg::Value(self.expr(b, x)),
                Arg::Function(name) => HostArg::Function(name.clone()),
            })
            .collect()
    }

    fn expr(&mut self, b: BlockId, x: &Expr) -> Value {
        let op = match x {
            Expr::Reg(loc) => Op::Get(*loc),
            Expr::Const(ty, value) => Op::Const { ty: *ty, value: *value },
            Expr::Load(ty, addr) => Op::Load { ty: *ty, addr: self.expr(b, addr), order: None },
            Expr::Global(g, ty) => Op::HostLoad { global: g.clone(), ty: *ty },
            Expr::Call(callee, args, ret) => {
                Op::HostCall { callee: callee.clone(), args: self.args(b, args), ret: Some(*ret) }
            }
            Expr::Bin(op, x, y) => Op::Bin(*op, self.expr(b, x), self.expr(b, y)),
            Expr::Cmp(p, x, y) => Op::Cmp(*p, self.expr(b, x), self.expr(b, y)),
            Expr::Select(c, x, y) => Op::Select(self.expr(b, c), self.expr(b, x), self.expr(b, y)),
            Expr::Cast(c, ty, x) => Op::Cast(*c, *ty, self.expr(b, x)),
            Expr::Bitcast(ty, x) => Op::Bitcast(*ty, self.expr(b, x)),
            Expr::FCast(c, ty, x) => Op::FCast(*c, *ty, self.expr(b, x)),
            Expr::FBin(op, x, y) => Op::FBin(*op, self.expr(b, x), self.expr(b, y)),
            Expr::Extract(x, lane) => Op::Extract(self.expr(b, x), *lane),
            Expr::Var(n) => {
                let (at, v) = self.vars.get(n).copied().expect("a Let before its Var");
                assert_eq!(at, b, "a Let's value is used in its own block alone");
                return v;
            }
        };
        self.e.func().push(b, op)
    }
}

/// Shorthands for writing edits.
impl Expr {
    /// X`n`'s value.
    pub fn x(n: u8) -> Expr {
        Expr::Reg(Loc::X(n))
    }

    /// A 64-bit constant.
    pub fn k64(v: u64) -> Expr {
        Expr::Const(Type::I64, u128::from(v))
    }

    /// A 32-bit constant.
    pub fn k32(v: u32) -> Expr {
        Expr::Const(Type::I32, u128::from(v))
    }

    /// An 8-bit constant.
    pub fn k8(v: u8) -> Expr {
        Expr::Const(Type::I8, u128::from(v))
    }

    /// `self op o`.
    pub fn bin(self, op: BinOp, o: Expr) -> Expr {
        Expr::Bin(op, Box::new(self), Box::new(o))
    }

    /// `self / o`, unsigned.
    pub fn udiv(self, o: Expr) -> Expr {
        self.bin(BinOp::UDiv, o)
    }

    /// `self | o`.
    pub fn or(self, o: Expr) -> Expr {
        self.bin(BinOp::Or, o)
    }

    /// `self & o`.
    pub fn and(self, o: Expr) -> Expr {
        self.bin(BinOp::And, o)
    }

    /// `self pred o`, an `i1`.
    pub fn cmp(self, p: Pred, o: Expr) -> Expr {
        Expr::Cmp(p, Box::new(self), Box::new(o))
    }

    /// `self ? a : b`.
    pub fn select(self, a: Expr, b: Expr) -> Expr {
        Expr::Select(Box::new(self), Box::new(a), Box::new(b))
    }

    /// Zero-extended to `ty`.
    pub fn zext(self, ty: Type) -> Expr {
        Expr::Cast(Cast::Zext, ty, Box::new(self))
    }

    /// Truncated to `ty`.
    pub fn trunc(self, ty: Type) -> Expr {
        Expr::Cast(Cast::Trunc, ty, Box::new(self))
    }

    /// The same bits as `ty`.
    pub fn bitcast(self, ty: Type) -> Expr {
        Expr::Bitcast(ty, Box::new(self))
    }

    /// Loaded from guest memory as `ty`.
    pub fn load(self, ty: Type) -> Expr {
        Expr::Load(ty, Box::new(self))
    }

    /// A V register's low single-precision float (its S register).
    pub fn s(n: u8) -> Expr {
        Expr::Extract(Box::new(Expr::Reg(Loc::V(n))), 0).trunc(Type::I32).bitcast(Type::F32)
    }

    /// A V register's value that holds the float `self` in its low 32 bits and zeros above (a write of S`n`).
    pub fn v_of_f32(self) -> Expr {
        self.bitcast(Type::I32).zext(Type::I128).bitcast(mir::V128)
    }

    /// The value of the host function `name`, called with `args`, returning `ret`.
    pub fn call(name: &str, args: Vec<Arg>, ret: Type) -> Expr {
        Expr::Call(Callee::Named(name.to_string()), args, ret)
    }
}

macro_rules! operator {
    ($trait:ident, $method:ident, $op:ident, $doc:literal) => {
        #[doc = $doc]
        impl std::ops::$trait for Expr {
            type Output = Expr;
            fn $method(self, o: Expr) -> Expr {
                self.bin(BinOp::$op, o)
            }
        }
    };
}

operator!(Add, add, Add, "`a + b`, of integers.");
operator!(Sub, sub, Sub, "`a - b`, of integers.");
operator!(Mul, mul, Mul, "`a * b`, of integers.");
operator!(Shl, shl, Shl, "`a << b`, of integers.");

impl Cond {
    /// `x pred y`.
    pub fn test(p: Pred, x: Expr, y: Expr) -> Cond {
        Cond::Test(p, x, y)
    }

    /// `self && o`.
    pub fn and(self, o: Cond) -> Cond {
        Cond::And(Box::new(self), Box::new(o))
    }

    /// `self || o`.
    pub fn or(self, o: Cond) -> Cond {
        Cond::Or(Box::new(self), Box::new(o))
    }
}

/// A function that runs under a wrapper of its name (`f_<entry>`), its own body lifted as [`Wrap::body_symbol`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wrap {
    /// The wrapper calls the body, then this code: a `return` site's hooks, after every way out of the body (a
    /// return, a tail call, an indirect jump all come back to the wrapper).
    Return(Vec<Stmt>),
    /// The wrapper calls the host function `host(Ctx *, A64Fn body)`, which calls the body itself.
    Host {
        /// The host function.
        host: String,
    },
}

impl Wrap {
    /// The symbol a wrapped function's body is lifted as: `f_1<entry>`, nine hex digits (no guest function's name).
    pub fn body_symbol(entry: u64) -> String {
        // (eight digits after the 1: an entry of more would make a name another entry's f_ could have)
        assert!(entry < 1 << 32, "a wrapped function's entry {entry:#x} is past 32 bits");
        format!("f_1{entry:08x}")
    }

    /// The wrapper of the body `f_1<entry>`, `f_<entry>`, as mid IR.
    pub fn wrapper(&self, entry: u64) -> mir::Function {
        let body = Wrap::body_symbol(entry);
        let mut w = Wrapper { f: mir::Function::new(entry) };
        w.f.wrapper = true;
        let b = w.f.start;
        let end = match self {
            Wrap::Return(after) => {
                w.f.push(
                    b,
                    mir::Op::HostCall { callee: mir::Callee::Named(body), args: vec![mir::HostArg::Ctx], ret: None },
                );
                let Ok(end) = emit(&mut w, b, after);
                end
            }
            Wrap::Host { host } => {
                w.f.push(
                    b,
                    mir::Op::HostCall {
                        callee: mir::Callee::Named(host.clone()),
                        args: vec![mir::HostArg::Ctx, mir::HostArg::Function(body)],
                        ret: None,
                    },
                );
                Some(b)
            }
        };
        if let Some(end) = end {
            w.f.terminate(end, mir::Term::Return);
        }
        w.f
    }
}

/// A wrapper's builder: its code has no guest instruction and no goto.
struct Wrapper {
    f: mir::Function,
}

impl Emit for Wrapper {
    type Error = std::convert::Infallible;
    fn func(&mut self) -> &mut mir::Function {
        &mut self.f
    }
    fn guest_block(&self) -> u64 {
        self.f.entry
    }
    fn edge(&mut self, to: Target) -> mir::Edge {
        unreachable!("a wrapper's code goes nowhere but back: {to:?}")
    }
    fn insn(&mut self, _: mir::BlockId) -> Result<(), Self::Error> {
        unreachable!("a wrapper has no instruction of its own")
    }
}
