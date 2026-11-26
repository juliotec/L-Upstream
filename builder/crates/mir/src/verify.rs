//! The IR's rules, checked (Function::verify).

use crate::*;
use std::collections::HashMap;

/// A broken rule: which block, and what.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{function}: {block}: {what}")]
pub struct VerifyError {
    /// The function's name.
    pub function: String,
    /// The block.
    pub block: BlockId,
    /// What is wrong.
    pub what: String,
}

struct Checker<'f> {
    f: &'f Function,
    /// A helper's body's parameters (`None`: a guest function).
    params: Option<&'f [Type]>,
    /// Each value's block and its place in it.
    at: HashMap<Value, (BlockId, usize)>,
}

impl Checker<'_> {
    fn operand(&self, b: BlockId, pos: usize, v: Value) -> Result<Type, String> {
        match self.at.get(&v) {
            Some(&(vb, vp)) if vb == b && vp < pos => self.f.ty(v).ok_or_else(|| format!("{v} has no value")),
            Some(&(vb, _)) if vb == b => Err(format!("{v} is used before it is defined")),
            Some(&(vb, _)) => Err(format!("{v} is defined in {vb}: a value is used only in its own block")),
            None => Err(format!("{v} is no instruction of this function")),
        }
    }

    fn of(&self, b: BlockId, pos: usize, v: Value, want: Type, role: &str) -> Result<(), String> {
        let ty = self.operand(b, pos, v)?;
        if ty == want { Ok(()) } else { Err(format!("{role} {v} is {ty}, not {want}")) }
    }

    fn same(&self, b: BlockId, pos: usize, x: Value, y: Value) -> Result<Type, String> {
        let (tx, ty) = (self.operand(b, pos, x)?, self.operand(b, pos, y)?);
        if tx == ty { Ok(tx) } else { Err(format!("{x} is {tx} and {y} is {ty}")) }
    }

    fn inst(&self, b: BlockId, pos: usize, v: Value) -> Result<(), String> {
        use Op::*;
        let i64 = Type::I64;
        let op = &self.f.inst(v).op;
        if self.params.is_some() && !op.is_pure() {
            return Err(format!("{v}: {op} in a helper (whose ops are pure)"));
        }
        match op {
            Const { ty, value } => {
                if ty.is_vector() || ty.is_float() {
                    return Err(format!("{v}: a constant of {ty} (constants are integers)"));
                }
                if value & !ty.mask() != 0 {
                    return Err(format!("{v}: {value:#x} does not fit {ty}"));
                }
            }
            Get(_) | Fence | Clrex | Counter | Call { .. } | SetjmpCall { .. } | Svc { .. } => {}
            HostLoad { ty, .. } => scalar_int(*ty, v)?,
            HostStore { value, .. } => scalar_int(self.operand(b, pos, *value)?, v)?,
            HostCall { args, ret, .. } => {
                for a in args {
                    if let crate::HostArg::Value(x) = a {
                        let t = self.operand(b, pos, *x)?;
                        if t.is_vector() || t == Type::I1 || t == Type::I128 {
                            return Err(format!("{v}: a host call's argument {x} of {t}"));
                        }
                    }
                }
                if let Some(t) = ret
                    && (t.is_vector() || *t == Type::I1 || *t == Type::I128)
                {
                    return Err(format!("{v}: a host call returning {t}"));
                }
            }
            Set(loc, x) => self.of(b, pos, *x, loc.ty(), &format!("the value set to {loc},"))?,
            Bin(op, x, y) => {
                let t = self.same(b, pos, *x, *y)?;
                int(t, v)?;
                if matches!(op, BinOp::Shl | BinOp::LShr | BinOp::AShr)
                    && let Some(k) = self.constant(*y)
                    && k >= u128::from(t.elem().bits())
                {
                    return Err(format!("{v}: a shift of {t} by {k}, not below its width"));
                }
            }
            Cmp(_, x, y) => int(self.same(b, pos, *x, *y)?, v)?,
            Un(op, x) => {
                let t = self.operand(b, pos, *x)?;
                int(t, v)?;
                if *op == UnOp::ByteSwap && (t.elem().bits() < 16 || t.elem().bits() % 16 != 0) {
                    return Err(format!("{v}: a byte swap of {t} (whole halfwords only)"));
                }
            }
            Select(c, x, y) => {
                let t = self.same(b, pos, *x, *y)?;
                let ct = self.operand(b, pos, *c)?;
                if ct != Type::I1 && ct != t.with_elem(Type::I1) {
                    return Err(format!("{v}: the condition {c} is {ct}, for {t}"));
                }
            }
            Cast(cast, to, x) => {
                let from = self.operand(b, pos, *x)?;
                int(from, v)?;
                int(*to, v)?;
                let ok = from.lanes() == to.lanes()
                    && match cast {
                        crate::Cast::Zext | crate::Cast::Sext => to.elem().bits() > from.elem().bits(),
                        crate::Cast::Trunc => to.elem().bits() < from.elem().bits(),
                    };
                if !ok {
                    return Err(format!("{v}: {cast:?} of {from} to {to}"));
                }
            }
            Bitcast(to, x) => {
                let from = self.operand(b, pos, *x)?;
                if from.bits() != to.bits() || from.elem() == Type::I1 || to.elem() == Type::I1 {
                    return Err(format!("{v}: a bitcast of {from} to {to}"));
                }
            }
            FBin(_, x, y) | FCmp(_, x, y) => float(self.same(b, pos, *x, *y)?, v)?,
            FUn(_, x) => float(self.operand(b, pos, *x)?, v)?,
            Fma(x, y, z) => {
                let t = self.same(b, pos, *x, *y)?;
                self.of(b, pos, *z, t, "the addend")?;
                float(t, v)?;
            }
            FCast(cast, to, x) => {
                let from = self.operand(b, pos, *x)?;
                let (fb, tb) = (from.elem().bits(), to.elem().bits());
                let ok = from.lanes() == to.lanes()
                    && match cast {
                        crate::FCast::Ext => from.is_float() && to.is_float() && tb > fb,
                        crate::FCast::Trunc => from.is_float() && to.is_float() && tb < fb,
                        crate::FCast::FromSigned | crate::FCast::FromUnsigned => {
                            from.is_int() && from.elem() != Type::I1 && to.is_float()
                        }
                        crate::FCast::ToSigned | crate::FCast::ToUnsigned => {
                            from.is_float() && to.is_int() && to.elem() != Type::I1
                        }
                    };
                if !ok {
                    return Err(format!("{v}: {cast:?} of {from} to {to}"));
                }
            }
            Extract(x, lane) => lane_of(self.operand(b, pos, *x)?, *lane, v)?,
            ExtractAt(x, i) => {
                let t = self.operand(b, pos, *x)?;
                let it = self.operand(b, pos, *i)?;
                if !t.is_vector() || it.is_vector() || !it.is_int() || it == Type::I1 {
                    return Err(format!("{v}: element {it} of {t}"));
                }
            }
            Insert(x, lane, e) => {
                let t = self.operand(b, pos, *x)?;
                lane_of(t, *lane, v)?;
                self.of(b, pos, *e, t.elem(), "the element")?;
            }
            Shuffle(x, y, mask) => {
                let t = self.same(b, pos, *x, *y)?;
                if !t.is_vector() || mask.len() < 2 || mask.len() > 64 {
                    return Err(format!("{v}: a shuffle of {t} by {} lanes", mask.len()));
                }
                if let Some(bad) = mask.iter().find(|&&m| u32::from(m) >= 2 * t.lanes()) {
                    return Err(format!("{v}: lane {bad} of two {t}"));
                }
            }
            Splat(to, x) => {
                if !to.is_vector() {
                    return Err(format!("{v}: a splat to {to}"));
                }
                self.of(b, pos, *x, to.elem(), "the element")?;
            }
            Reduce(_, x) => {
                let t = self.operand(b, pos, *x)?;
                if !t.is_vector() || !t.is_int() {
                    return Err(format!("{v}: a reduction of {t}"));
                }
            }
            Lookup(_, x) => {
                let t = self.operand(b, pos, *x)?;
                if t.is_vector() || !t.is_int() {
                    return Err(format!("{v}: a table index of {t}"));
                }
            }
            Load { addr, ty, order } => {
                self.of(b, pos, *addr, i64, "the address")?;
                accessed(*ty, order.is_some(), v)?;
            }
            Mark { addr, bytes } => {
                self.of(b, pos, *addr, i64, "the address")?;
                if *bytes == 0 {
                    return Err(format!("{v}: a mark of no bytes"));
                }
            }
            Ldxr { addr, bytes } | Ldxp { addr, bytes } => {
                self.of(b, pos, *addr, i64, "the address")?;
                exclusive_size(*bytes, matches!(op, Ldxp { .. }), v)?;
            }
            Store { addr, value, order, .. } => {
                self.of(b, pos, *addr, i64, "the address")?;
                accessed(self.operand(b, pos, *value)?, order.is_some(), v)?;
            }
            AtomicRmw { addr, value, .. } => {
                self.of(b, pos, *addr, i64, "the address")?;
                accessed(self.operand(b, pos, *value)?, true, v)?;
            }
            CmpXchg { addr, expected, new } => {
                self.of(b, pos, *addr, i64, "the address")?;
                let t = self.same(b, pos, *expected, *new)?;
                if t != Type::I128 {
                    accessed(t, true, v)?;
                }
            }
            Stxr { addr, value, bytes } => {
                self.of(b, pos, *addr, i64, "the address")?;
                self.of(b, pos, *value, i64, "the value")?;
                exclusive_size(*bytes, false, v)?;
            }
            Stxp { addr, lo, hi, bytes } => {
                self.of(b, pos, *addr, i64, "the address")?;
                self.of(b, pos, *lo, i64, "the low half")?;
                self.of(b, pos, *hi, i64, "the high half")?;
                exclusive_size(*bytes, true, v)?;
            }
            CallIndirect { target, .. } => self.of(b, pos, *target, i64, "the target")?,
            Param { index, ty } => match self.params {
                None => return Err(format!("{v}: a parameter outside a helper")),
                Some(params) if params.get(usize::from(*index)) != Some(ty) => {
                    return Err(format!("{v}: parameter {index} of {ty}, of a helper of ({params:?})"));
                }
                Some(_) => {}
            },
            FClass(_, x) => float(self.operand(b, pos, *x)?, v)?,
            Checked { fast, slow, helper, args } => {
                if self.params.is_some() {
                    return Err(format!("{v}: a helper's call in a helper"));
                }
                let Some(h) = self.f.helpers.get(helper.0 as usize) else {
                    return Err(format!("{v}: helper {} does not exist", helper.0));
                };
                let t = self.operand(b, pos, *fast)?;
                if t != h.ret() {
                    return Err(format!("{v}: the fast value {fast} is {t}, {} returns {}", h.name, h.ret()));
                }
                self.of(b, pos, *slow, Type::I1, "the slow path's condition")?;
                if args.len() != h.params.len() {
                    return Err(format!("{v}: {} arguments to {}, of {}", args.len(), h.name, h.params.len()));
                }
                for (a, &want) in args.iter().zip(&h.params) {
                    self.of(b, pos, *a, want, &format!("an argument of {},", h.name))?;
                }
            }
        }
        Ok(())
    }

    /// The value of `v` when it is a constant, or a vector of one constant.
    fn constant(&self, v: Value) -> Option<u128> {
        match self.f.inst(v).op {
            Op::Const { value, .. } => Some(value),
            Op::Splat(_, x) => self.constant(x),
            _ => None,
        }
    }

    fn term(&self, b: BlockId, end: usize, term: &Term) -> Result<(), String> {
        match term {
            Term::Branch { cond, .. } => self.of(b, end, *cond, Type::I1, "the condition")?,
            Term::Switch { value, cases } => {
                self.of(b, end, *value, Type::I64, "the target")?;
                let mut seen = std::collections::HashSet::new();
                if let Some((a, _)) = cases.iter().find(|(a, _)| !seen.insert(*a)) {
                    return Err(format!("a switch with two cases of {a:#x}"));
                }
            }
            Term::IndirectJump(value) => self.of(b, end, *value, Type::I64, "the target")?,
            Term::Jump(_) | Term::Return | Term::Trap { .. } => {}
        }
        for e in term.edges() {
            if let Dest::Block(to) = e.to
                && to.0 as usize >= self.f.block_count()
            {
                return Err(format!("an edge to {to}, which does not exist"));
            }
            if let Dest::Piece { piece, .. } = e.to {
                let Some(own) = &self.f.pieces else {
                    return Err("an edge to another piece, in a function that is no piece".into());
                };
                if piece == 0 || piece == own.index {
                    return Err(format!("an edge to piece {piece}, from piece {} (pieces 1 on, another)", own.index));
                }
            }
        }
        Ok(())
    }
}

/// `t` must be what a load or store moves: anything but flags; a scalar integer of 8 to 64 bits when it is atomic.
fn accessed(t: Type, atomic: bool, v: Value) -> Result<(), String> {
    if t.elem() == Type::I1 {
        return Err(format!("{v}: an access of {t}"));
    }
    if atomic && !matches!(t, Type::I8 | Type::I16 | Type::I32 | Type::I64) {
        return Err(format!("{v}: an atomic access of {t} (scalar integers of 8 to 64 bits)"));
    }
    Ok(())
}

/// An exclusive access's size: 1, 2, 4 or 8 bytes, or each half of a pair (`pair`) 4 or 8.
fn exclusive_size(bytes: u32, pair: bool, v: Value) -> Result<(), String> {
    let ok = if pair { matches!(bytes, 4 | 8) } else { matches!(bytes, 1 | 2 | 4 | 8) };
    if ok {
        Ok(())
    } else {
        Err(format!("{v}: an exclusive access of {bytes} bytes{}", if pair { " each" } else { "" }))
    }
}

/// `t` must be of integer elements.
fn int(t: Type, v: Value) -> Result<(), String> {
    if t.is_int() { Ok(()) } else { Err(format!("{v}: an integer operation of {t}")) }
}

/// `t` must be a scalar integer of 8 to 64 bits (what a host global holds).
fn scalar_int(t: Type, v: Value) -> Result<(), String> {
    if matches!(t, Type::I8 | Type::I16 | Type::I32 | Type::I64) {
        Ok(())
    } else {
        Err(format!("{v}: a host global of {t}"))
    }
}

/// `t` must be of float elements.
fn float(t: Type, v: Value) -> Result<(), String> {
    if t.is_float() { Ok(()) } else { Err(format!("{v}: a float operation of {t}")) }
}

/// `lane` must be an element of the vector type `t`.
fn lane_of(t: Type, lane: u8, v: Value) -> Result<(), String> {
    if t.is_vector() && u32::from(lane) < t.lanes() { Ok(()) } else { Err(format!("{v}: lane {lane} of {t}")) }
}

/// The pieces of one split function, `pieces[i]` piece `i`, each verified: their edges to each other must name a piece
/// and one of its entry blocks.
pub(crate) fn verify_pieces(pieces: &[Function]) -> Result<(), VerifyError> {
    for f in pieces {
        for (id, b) in f.blocks() {
            for e in b.term.iter().flat_map(Term::edges) {
                let Dest::Piece { piece, entry } = e.to else { continue };
                let entries = pieces.get(piece as usize).map(|p| p.entries.len().max(1));
                if entries.is_none_or(|n| entry as usize >= n) {
                    return Err(VerifyError {
                        function: f.name(),
                        block: id,
                        what: format!("an edge to entry {entry} of piece {piece}, of {} pieces", pieces.len()),
                    });
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn verify(f: &Function) -> Result<(), VerifyError> {
    check(f, None)?;
    if let Some(&bad) = f.entries.iter().find(|b| b.0 as usize >= f.block_count()) {
        return Err(VerifyError { function: f.name(), block: bad, what: "an entry block that does not exist".into() });
    }
    for (_, h) in f.helpers() {
        verify_helper(h).map_err(|e| VerifyError { function: format!("{} (helper {})", f.name(), e.function), ..e })?;
    }
    Ok(())
}

/// A helper's rules: one block of pure ops over its parameters, ending in a return, its result a value of that block.
fn verify_helper(h: &Helper) -> Result<(), VerifyError> {
    let f = &h.body;
    let fail = |what: String| VerifyError { function: h.name.clone(), block: f.start, what };
    if f.block_count() != 1 || f.start != BlockId(0) {
        return Err(fail(format!("{} blocks (a helper's body is one)", f.block_count())));
    }
    if h.params.len() > usize::from(u8::MAX) {
        return Err(fail(format!("{} parameters", h.params.len())));
    }
    check(f, Some(&h.params)).map_err(|e| VerifyError { function: h.name.clone(), ..e })?;
    if f.block(f.start).term != Some(Term::Return) {
        return Err(fail("the body does not end in a return".into()));
    }
    if !f.block(f.start).insts.contains(&h.result) || f.ty(h.result).is_none() {
        return Err(fail(format!("the result {} is no value of the body", h.result)));
    }
    Ok(())
}

fn check(f: &Function, params: Option<&[Type]>) -> Result<(), VerifyError> {
    let mut at = HashMap::new();
    for (id, b) in f.blocks() {
        for (pos, &v) in b.insts.iter().enumerate() {
            at.insert(v, (id, pos));
        }
    }
    let checker = Checker { f, params, at };
    for (id, b) in f.blocks() {
        let fail = |what: String| VerifyError { function: f.name(), block: id, what };
        for (pos, &v) in b.insts.iter().enumerate() {
            checker.inst(id, pos, v).map_err(fail)?;
        }
        let term = b.term.as_ref().ok_or_else(|| fail("the block has no end".into()))?;
        checker.term(id, b.insts.len(), term).map_err(fail)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_each_kind_of_fault() {
        let mut f = Function::new(0x1000);
        let b = f.start;
        let x = f.push(b, Op::Get(Loc::X(1)));
        let c = f.push(b, Op::Const { ty: Type::I32, value: 1 });
        f.push(b, Op::Bin(BinOp::Add, x, c));
        f.terminate(b, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("is i64 and"));

        let mut f = Function::new(0x1000);
        let b0 = f.start;
        let b1 = f.add_block(0x1004);
        let x = f.push(b0, Op::Get(Loc::X(1)));
        f.terminate(b0, Term::Jump(Edge::new(Dest::Block(b1), false)));
        f.push(b1, Op::Set(Loc::X(0), x));
        f.terminate(b1, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("used only in its own block"));

        let mut f = Function::new(0x1000);
        let b = f.start;
        f.terminate(b, Term::Jump(Edge::new(Dest::Block(BlockId(9)), false)));
        assert!(f.verify().unwrap_err().what.contains("does not exist"));

        let mut f = Function::new(0x1000);
        f.push(f.start, Op::Const { ty: Type::I8, value: 0x100 });
        f.terminate(f.start, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("does not fit"));

        let f = Function::new(0x1000);
        assert!(f.verify().unwrap_err().what.contains("no end"));

        let mut f = Function::new(0x1000);
        f.push(f.start, Op::Const { ty: Type::F32, value: 0 });
        f.terminate(f.start, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("constants are integers"));

        let mut f = Function::new(0x1000);
        let v = f.push(f.start, Op::Get(Loc::V(3)));
        f.push(f.start, Op::Bitcast(Type::Vec(2, Elem::F32), v));
        f.terminate(f.start, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("a bitcast"));

        let mut f = Function::new(0x1000);
        let v = f.push(f.start, Op::Get(Loc::V(3)));
        let x = f.push(f.start, Op::Bitcast(Type::Vec(4, Elem::F32), v));
        f.push(f.start, Op::Extract(x, 4));
        f.terminate(f.start, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("lane 4"));

        let mut f = Function::new(0x1000);
        let v = f.push(f.start, Op::Get(Loc::V(3)));
        f.push(f.start, Op::Bin(BinOp::Add, v, v));
        let x = f.push(f.start, Op::Bitcast(Type::Vec(2, Elem::F64), v));
        f.push(f.start, Op::Bin(BinOp::Add, x, x));
        f.terminate(f.start, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("an integer operation"));
    }

    /// The error of a function of the ops `make` appends to its start, then a return.
    fn fault(make: impl FnOnce(&mut Function, BlockId)) -> String {
        let mut f = Function::new(0x1000);
        let b = f.start;
        make(&mut f, b);
        if !f.is_terminated(b) {
            f.terminate(b, Term::Return);
        }
        f.verify().expect_err("a malformed function").what
    }

    #[test]
    fn catches_what_llvm_would_refuse_or_make_poison_of() {
        let k = |f: &mut Function, b, ty, value| f.push(b, Op::Const { ty, value });
        // a shift by its width, scalar or splat
        assert!(
            fault(|f, b| {
                let x = k(f, b, Type::I32, 1);
                let n = k(f, b, Type::I32, 32);
                f.push(b, Op::Bin(BinOp::Shl, x, n));
            })
            .contains("not below its width")
        );
        assert!(
            fault(|f, b| {
                let x = f.push(b, Op::Get(Loc::V(0)));
                let n = k(f, b, Type::I64, 64);
                let n = f.push(b, Op::Splat(V128, n));
                f.push(b, Op::Bin(BinOp::LShr, x, n));
            })
            .contains("not below its width")
        );
        // atomics of what is no scalar integer of 8 to 64 bits (CmpXchg: or of 128)
        assert!(
            fault(|f, b| {
                let a = f.push(b, Op::Get(Loc::X(0)));
                let v = f.push(b, Op::Get(Loc::V(0)));
                f.push(b, Op::AtomicRmw { op: Rmw::Max, addr: a, value: v, order: Order::SeqCst });
            })
            .contains("an atomic access")
        );
        assert!(
            fault(|f, b| {
                let a = f.push(b, Op::Get(Loc::X(0)));
                let x = k(f, b, Type::I32, 0);
                let x = f.push(b, Op::Bitcast(Type::F32, x));
                f.push(b, Op::CmpXchg { addr: a, expected: x, new: x });
            })
            .contains("an atomic access")
        );
        // a load or store of flags
        assert!(
            fault(|f, b| {
                let a = f.push(b, Op::Get(Loc::X(0)));
                f.push(b, Op::Load { ty: Type::I1, addr: a, order: None });
            })
            .contains("an access of i1")
        );
        assert!(
            fault(|f, b| {
                let a = f.push(b, Op::Get(Loc::X(0)));
                let c = f.push(b, Op::Get(Loc::Flag(Flag::C)));
                f.push(b, Op::Store { addr: a, value: c, space: Space::Guest, order: None });
            })
            .contains("an access of i1")
        );
        // exclusives of no size the monitor has, a mark of nothing
        assert!(
            fault(|f, b| {
                let a = f.push(b, Op::Get(Loc::X(0)));
                f.push(b, Op::Ldxr { addr: a, bytes: 3 });
            })
            .contains("an exclusive access of 3 bytes")
        );
        assert!(
            fault(|f, b| {
                let a = f.push(b, Op::Get(Loc::X(0)));
                f.push(b, Op::Stxp { addr: a, bytes: 2, lo: a, hi: a });
            })
            .contains("2 bytes each")
        );
        assert!(
            fault(|f, b| {
                let a = f.push(b, Op::Get(Loc::X(0)));
                f.push(b, Op::Mark { addr: a, bytes: 0 });
            })
            .contains("a mark of no bytes")
        );
        // a byte swap of a byte
        assert!(
            fault(|f, b| {
                let x = k(f, b, Type::I8, 1);
                f.push(b, Op::Un(UnOp::ByteSwap, x));
            })
            .contains("a byte swap")
        );
        // a switch with one address twice
        assert!(
            fault(|f, b| {
                let x = f.push(b, Op::Get(Loc::X(0)));
                let to = Edge::new(Dest::BadTarget(0x2000), false);
                f.terminate(b, Term::Switch { value: x, cases: vec![(0x2000, to), (0x2000, to)] });
            })
            .contains("two cases of 0x2000")
        );
        // a piece's edge to itself or to piece 0
        for piece in [0, 1] {
            let what = fault(|f, b| {
                f.pieces = Some(Pieces { base: "f_1000".into(), index: 1 });
                f.terminate(b, Term::Jump(Edge::new(Dest::Piece { piece, entry: 0 }, false)));
            });
            assert!(what.contains(&format!("an edge to piece {piece}")), "{what}");
        }
    }

    #[test]
    fn pieces_name_pieces_and_entries_there_are() {
        let piece = |index: u32, to: u32, entry: u32| {
            let mut f = Function::new(0x1000);
            f.pieces = Some(Pieces { base: "f_1000".into(), index });
            f.terminate(f.start, Term::Jump(Edge::new(Dest::Piece { piece: to, entry }, false)));
            f.verify().unwrap();
            f
        };
        verify_pieces(&[piece(0, 1, 0), piece(1, 2, 0), piece(2, 1, 0)]).unwrap();
        let e = verify_pieces(&[piece(0, 1, 0), piece(1, 3, 0)]).unwrap_err();
        assert!(e.what.contains("piece 3, of 2 pieces"), "{e}");
        let e = verify_pieces(&[piece(0, 1, 0), piece(1, 2, 0), piece(2, 1, 1)]); // (piece 1 has its start alone)
        assert!(e.is_err());
    }

    /// A helper returning its first parameter, of `ty`, with `extra` in its body.
    fn helper(ty: Type, extra: Option<Op>) -> Helper {
        let mut body = Function::new(0);
        let p = body.push(body.start, Op::Param { index: 0, ty });
        if let Some(op) = extra {
            body.push(body.start, op);
        }
        body.terminate(body.start, Term::Return);
        Helper { name: "h".into(), params: vec![ty], body, result: p }
    }

    #[test]
    fn checks_fast_paths_and_helpers() {
        let with = |h: Helper, arg_ty: Type| {
            let mut f = Function::new(0x1000);
            let id = f.add_helper(h);
            let b = f.start;
            let v = f.push(b, Op::Get(Loc::V(0)));
            let x = f.push(b, Op::Bitcast(Type::Vec(4, Elem::F32), v));
            let arg = if arg_ty == Type::Vec(4, Elem::F32) { x } else { v };
            let nan = f.push(b, Op::FCmp(FPred::Uno, x, x));
            let any = f.push(b, Op::Reduce(Reduce::Or, nan));
            f.push(b, Op::Checked { fast: x, slow: any, helper: id, args: vec![arg] });
            f.terminate(b, Term::Return);
            f.verify()
        };
        let f4 = Type::Vec(4, Elem::F32);
        with(helper(f4, None), f4).unwrap();
        assert!(with(helper(f4, None), V128).unwrap_err().what.contains("an argument of h"));
        assert!(with(helper(Type::F32, None), Type::F32).unwrap_err().what.contains("the fast value"));
        let e = with(helper(f4, Some(Op::Get(Loc::X(0)))), f4).unwrap_err();
        assert!(e.what.contains("in a helper (whose ops are pure)"), "{e}");
        let e = with(helper(f4, Some(Op::Param { index: 1, ty: f4 })), f4).unwrap_err();
        assert!(e.what.contains("parameter 1"), "{e}");

        let mut f = Function::new(0x1000);
        f.push(f.start, Op::Param { index: 0, ty: Type::I64 });
        f.terminate(f.start, Term::Return);
        assert!(f.verify().unwrap_err().what.contains("outside a helper"));
    }
}
