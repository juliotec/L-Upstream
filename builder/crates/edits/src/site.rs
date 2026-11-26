//! Each placed hook site (the `sites` crate's [`Placed`]) as code, by its form (docs/dev/reference/hooks.md#forms),
//! over the host's ABI (game/sites.h): the site's flag byte `hos_site_on_<ident>`, its callback slot
//! `hos_site_fn_<ident>` (an `int (*)(Ctx *)`), the replaced call's `hos_site_callee`, and a data rule's value.

use crate::code::{Arg, Cond, Expr, Stmt, Target};
use mir::{Callee, FBinOp, Global, Loc, Pred, Type};
use sites::{Hook, Placed, Point, RegClass, Rule};

/// The code a site adds, by where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteCode {
    /// At the function's entry, before anything of its own.
    Entry(Vec<Stmt>),
    /// After the function returns, by any way out.
    Return(Vec<Stmt>),
    /// After the instruction at `.0`.
    After(u64, Vec<Stmt>),
    /// Before the instruction at `.0`, on every way into it (a `jump` site's too).
    Before(u64, Vec<Stmt>),
    /// In place of the instruction at `.0` ([`Stmt::Insn`] where it keeps it).
    Replace(u64, Vec<Stmt>),
    /// Before the conditional branch at `.0`'s own test: [`Target::Taken`] takes it.
    Branch(u64, Vec<Stmt>),
    /// The conditional branch at `.0` taken only when its own test holds and then this holds.
    Allow(u64, Cond),
}

/// The site's flag byte tested: whether its callback is on (`on`), or off.
fn flag(ident: &str, on: bool) -> Cond {
    let pred = if on { Pred::Ne } else { Pred::Eq };
    Cond::test(pred, Expr::Global(Global::new(format!("hos_site_on_{ident}")), Type::I8), Expr::k8(0))
}

fn on(ident: &str) -> Cond {
    flag(ident, true)
}

fn slot(ident: &str) -> Callee {
    Callee::Slot(Global::new(format!("hos_site_fn_{ident}")))
}

/// The callback called for what it does.
fn call(ident: &str) -> Stmt {
    Stmt::Call(slot(ident), vec![Arg::Ctx], Some(Type::I32))
}

/// The callback's answer is not 0.
fn says_yes(ident: &str) -> Cond {
    Cond::test(Pred::Ne, Expr::Call(slot(ident), vec![Arg::Ctx], Type::I32), Expr::k32(0))
}

/// `if (on) fn(c);`
fn tell(ident: &str) -> Vec<Stmt> {
    vec![Stmt::If(on(ident), vec![call(ident)], vec![])]
}

/// `if (on && fn(c)) <then>`
fn when_yes(ident: &str, then: Stmt) -> Vec<Stmt> {
    vec![Stmt::If(on(ident).and(says_yes(ident)), vec![then], vec![])]
}

/// The register a rule's argument of this name is in.
fn arg(p: &Placed, name: &str, class: RegClass) -> u8 {
    p.args
        .iter()
        .find(|a| a.name.as_deref() == Some(name) && a.class == class)
        .map(|a| a.reg)
        .unwrap_or_else(|| panic!("{}: the table's check gives a rule its arguments ({name})", p.name))
}

/// A data rule's code: straight-line, its value read once (docs/dev/reference/hooks.md#inline-data-rules).
fn rule(p: &Placed, rule: Rule, symbol: &str) -> Vec<Stmt> {
    let g = Global::new(symbol);
    match rule {
        Rule::MulF32 | Rule::ScaleF32 => {
            // (the value, a float's bits; 0 is off, and for scale_f32 1.0 too: S<out> then keeps its value)
            let out = arg(p, "out", RegClass::S);
            let input = if rule == Rule::MulF32 { arg(p, "in", RegClass::S) } else { out };
            let k = Expr::Var(0);
            let mut active = k.clone().cmp(Pred::Ne, Expr::k32(0));
            if rule == Rule::ScaleF32 {
                active = active.and(k.clone().cmp(Pred::Ne, Expr::k32(0x3f80_0000)));
            }
            let product = Expr::FBin(FBinOp::Mul, Box::new(Expr::s(input)), Box::new(k.bitcast(Type::F32)));
            vec![
                Stmt::Let(0, Expr::Global(g, Type::I32)),
                Stmt::Set(Loc::V(out), active.select(product.v_of_f32(), Expr::Reg(Loc::V(out)))),
            ]
        }
        Rule::NoteU64 => vec![Stmt::SetGlobal(g, Expr::x(arg(p, "what", RegClass::X)))],
    }
}

/// The code of a placed site. `call_at` says whether the instruction at an address is a call (BL, BLR): a replaced
/// call's hook sets X30 first.
pub fn code(p: &Placed, call_at: impl Fn(u64) -> bool) -> SiteCode {
    let i = p.ident.as_str();
    if let Hook::Rule { rule: r, symbol, .. } = &p.hook {
        let c = rule(p, *r, symbol);
        return match p.point {
            Point::Entry { .. } => SiteCode::Entry(c),
            Point::Return => SiteCode::Return(c),
            Point::After { pc } => SiteCode::After(pc, c),
            Point::Before { pc } => SiteCode::Before(pc, c),
            // (the table's check keeps rules to the four forms above)
            _ => unreachable!("{}: a rule at {:?}", p.name, p.point),
        };
    }
    match p.point {
        Point::Entry { can_return: false } => SiteCode::Entry(tell(i)),
        Point::Entry { can_return: true } => SiteCode::Entry(when_yes(i, Stmt::Return)),
        Point::Return => SiteCode::Return(tell(i)),
        Point::After { pc } => SiteCode::After(pc, tell(i)),
        Point::Before { pc } => SiteCode::Before(pc, tell(i)),
        Point::Jump { pc, to } => SiteCode::Before(pc, when_yes(i, Stmt::Goto(Target::Pc(to)))),
        Point::Branch { pc } => SiteCode::Branch(pc, when_yes(i, Stmt::Goto(Target::Taken))),
        Point::Allow { pc } => SiteCode::Allow(pc, flag(i, false).or(says_yes(i))),
        Point::Replace { pc, callee } => SiteCode::Replace(pc, replace(i, pc, callee, call_at(pc))),
    }
}

/// `if (on) { [x30 = return address; hos_site_callee = callee;] fn(c); } else { the instruction }`: for a call, X30 its
/// return address first, as the call would set it, and for a bl the function the lift calls there (past PLT stubs) on
/// the thread, for the callback to call by what this build's code calls.
fn replace(i: &str, pc: u64, callee: Option<u64>, is_call: bool) -> Vec<Stmt> {
    let mut on_code = Vec::new();
    if is_call {
        on_code.push(Stmt::Set(Loc::X(30), Expr::k64(pc + 4)));
    }
    if let Some(c) = callee {
        on_code.push(Stmt::SetGlobal(Global::thread_local("hos_site_callee"), Expr::k64(c)));
    }
    on_code.push(call(i));
    vec![Stmt::If(on(i), on_code, vec![Stmt::Insn])]
}
