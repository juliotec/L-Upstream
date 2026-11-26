//! Each found site checked against the lift and placed: its function is one the lift found and the one the table
//! names, its instruction is of the kind its form needs, no two sites claim the same place in a way that cannot both
//! hold. The result is data for the mid IR, one [`Placed`] per site: where its hook goes and what it calls or does.

use crate::ResolveError;
use crate::code::{InsnKind, Lift};
use crate::host::HostList;
use crate::resolve::Found;
use crate::table::{Arg, Form, Rule, Site};
use std::collections::{BTreeMap, BTreeSet};

/// Where a site's hook goes, by form. `pc` is the site's instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    /// At the function's entry, before anything of its own; `can_return`: a non-zero answer returns at once.
    Entry {
        /// Whether the answer may return from the function.
        can_return: bool,
    },
    /// After the function returns, by any way out.
    Return,
    /// After the plain instruction.
    After {
        /// The instruction.
        pc: u64,
    },
    /// Before the instruction, on every way into it.
    Before {
        /// The instruction.
        pc: u64,
    },
    /// In place of the instruction while the site is on. For a replaced `bl`, `callee` is the function it calls (past
    /// PLT stubs), which the host's callback is told (`hos_site_callee`).
    Replace {
        /// The instruction.
        pc: u64,
        /// The function a replaced `bl` calls.
        callee: Option<u64>,
    },
    /// Before the conditional branch: a non-zero answer takes it.
    Branch {
        /// The branch.
        pc: u64,
    },
    /// The conditional branch is taken only when its own test holds and then the answer is non-zero.
    Allow {
        /// The branch.
        pc: u64,
    },
    /// Before the instruction: a non-zero answer jumps to `to`, in the same function.
    Jump {
        /// The instruction.
        pc: u64,
        /// Where it jumps.
        to: u64,
    },
}

impl Point {
    /// The site's instruction, for the forms inside the function.
    pub fn pc(&self) -> Option<u64> {
        match *self {
            Point::Entry { .. } | Point::Return => None,
            Point::After { pc }
            | Point::Before { pc }
            | Point::Replace { pc, .. }
            | Point::Branch { pc }
            | Point::Allow { pc }
            | Point::Jump { pc, .. } => Some(pc),
        }
    }
}

/// What the hook does at its point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hook {
    /// Test the site's flag byte (`hos_site_on_<ident>`) and call its slot (`hos_site_fn_<ident>`) with the context.
    Callback,
    /// An inline data rule, reading or writing the host's value `symbol` (no flag, no call).
    Rule {
        /// The rule.
        rule: Rule,
        /// The host value's name.
        value: String,
        /// The C variable that holds it.
        symbol: String,
    },
}

/// One placed site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Its name.
    pub name: String,
    /// Its C identifier.
    pub ident: String,
    /// Its function.
    pub entry: u64,
    /// Where its hook goes.
    pub point: Point,
    /// What the hook does.
    pub hook: Hook,
    /// The registers its callback reads.
    pub args: Vec<Arg>,
    /// The functions its instruction is lifted in (its own, and any whose body runs into it); empty for entry and
    /// return. Each gets the hook, and keeps its registers in the context there.
    pub holders: Vec<u64>,
}

impl Placed {
    /// The functions its hook goes in: its holders, or for an entry or return site its own function.
    pub fn functions(&self) -> Vec<u64> {
        if self.holders.is_empty() { vec![self.entry] } else { self.holders.clone() }
    }
}

/// Every placed site, and what placing them asks of the lift.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    /// The sites, in the table's order (the order of several hooks at one instruction).
    pub sites: Vec<Placed>,
    /// Labels the sites add: a jump's target, in each function its code is lifted in.
    pub labels: BTreeMap<u64, BTreeSet<u64>>,
    /// A line per site saying where it went and how it was found, and a summary.
    pub log: Vec<String>,
}

/// Where control goes from the instruction at `pc`, inside the function.
fn successors(f: &crate::code::Function<'_>, lift: &Lift<'_>, pc: u64) -> Vec<u64> {
    let next = match f.kind(pc) {
        Some(InsnKind::B(t)) => vec![t],
        Some(InsnKind::BCond(t) | InsnKind::Cbz(t) | InsnKind::Tbz(t)) => vec![pc + 4, t],
        Some(InsnKind::Br(_)) => lift.jump_table(pc).to_vec(),
        Some(InsnKind::Ret | InsnKind::Trap) | None => Vec::new(),
        Some(_) => vec![pc + 4],
    };
    next.into_iter().filter(|&t| f.contains(t)).collect()
}

/// Whether `pc` can be reached again from itself inside the function.
fn in_loop(f: &crate::code::Function<'_>, lift: &Lift<'_>, pc: u64) -> bool {
    let mut seen = BTreeSet::new();
    let mut todo = successors(f, lift, pc);
    while let Some(t) = todo.pop() {
        if t == pc {
            return true;
        }
        if seen.insert(t) {
            todo.extend(successors(f, lift, t));
        }
    }
    false
}

/// What kind of instruction a form needs, and its words: `None` for any.
/// A test of an instruction's kind, and the words for what it accepts.
type Needs = (fn(InsnKind) -> bool, &'static str);

fn needs(form: Form) -> Option<Needs> {
    match form {
        Form::After => Some((|k| k == InsnKind::Op, "a plain instruction")),
        Form::Replace => {
            Some((|k| matches!(k, InsnKind::Op | InsnKind::Bl(_) | InsnKind::Blr(_)), "a plain instruction or a call"))
        }
        Form::Branch | Form::Allow => Some((InsnKind::is_conditional, "a conditional branch")),
        Form::Before | Form::Jump | Form::Entry | Form::Return => None,
    }
}

/// The line after the placed sites: how many, each found exactly once, and how they were found.
pub fn summary(sites: &[Site], found: &Found) -> String {
    let mut ways: Vec<(&str, usize)> = Vec::new();
    for s in sites.iter().filter(|s| s.code().is_some()) {
        let how = found.get(&s.name).and_then(|l| l.how.as_deref()).unwrap_or("signature");
        let key = if how.contains("point:") {
            "an anchor in the function"
        } else if how.contains("fingerprint") {
            "a fingerprint"
        } else if how.starts_with("in ") {
            "another site's function"
        } else {
            "signature"
        };
        match ways.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => ways.push((key, 1)),
        }
    }
    ways.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let total: usize = ways.iter().map(|(_, n)| n).sum();
    let each: Vec<String> = ways.iter().map(|(k, n)| format!("{n} by {k}")).collect();
    format!("hooks: {total} site(s), each found exactly once and placed ({})", each.join(", "))
}

/// Each found site checked against the lift and placed. `found`: [`crate::resolve`]'s and [`crate::locate`]'s; `host`:
/// the host's list, for the rules' values. Every site that does not fit is named.
pub fn place(sites: &[Site], lift: &Lift<'_>, found: &Found, host: &HostList) -> Result<Placement, ResolveError> {
    let mut out = Placement::default();
    let mut bad = Vec::new();
    let (mut after, mut before, mut replaced) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let (mut branches, mut allows, mut jumps) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for s in sites {
        let Some(c) = s.code() else { continue };
        let Some(at) = found.get(&s.name) else {
            bad.push(format!("  {}: not found", s.name));
            continue;
        };
        let entry = at.addr.wrapping_add_signed(-c.signature_offset);
        let how = at.how.clone().unwrap_or_else(|| "signature".into());
        let Some(f) = lift.function(entry) else {
            bad.push(format!("  {}: its signature puts the function at {entry:#x}, where the lift found none", s.name));
            continue;
        };
        if entry != c.function {
            bad.push(format!(
                "  {}: its {} finds f_{entry:08x}, the table names f_{:08x}",
                s.name,
                how.split(';').next().unwrap_or_default(),
                c.function
            ));
            continue;
        }
        let hook = match &c.rule {
            None => Hook::Callback,
            Some(r) => Hook::Rule {
                rule: r.rule,
                value: r.value.clone(),
                symbol: host
                    .values
                    .get(&r.value)
                    .map_or_else(|| format!("hos_value_{}", crate::table::ident(&r.value)), |v| v.symbol.clone()),
            },
        };
        let rule_note = c.rule.as_ref().map(|r| format!(" (inline rule {})", r.rule.name())).unwrap_or_default();
        let mut placed = Placed {
            name: s.name.clone(),
            ident: s.ident(),
            entry,
            point: Point::Return,
            hook,
            args: s.args.clone(),
            holders: Vec::new(),
        };
        if !c.form.at_offset() {
            placed.point =
                if c.form == Form::Entry { Point::Entry { can_return: c.can_return } } else { Point::Return };
            out.log.push(format!("hooks: {} at f_{entry:08x}'s {}{rule_note} [by {how}]", s.name, c.form.name()));
            out.sites.push(placed);
            continue;
        }
        let offset = at.offset.or(c.offset).unwrap_or(0);
        let pc = entry + offset;
        let here = format!("f_{entry:08x}+{offset:#x}");
        let Some(kind) = f.kind(pc) else {
            bad.push(format!("  {}: {here} is not an instruction of the function", s.name));
            continue;
        };
        if let Some((fits, what)) = needs(c.form)
            && !fits(kind)
        {
            bad.push(format!(
                "  {}: {here} is a {}, not {what}: at = \"{}\" goes at one (docs/dev/reference/hooks.md)",
                s.name,
                kind.name(),
                c.form.name()
            ));
            continue;
        }
        let holders = lift.holders(pc);
        placed.point = match c.form {
            Form::Jump => {
                let to = c.to.unwrap_or(0);
                let target = entry + to;
                if !f.contains(target) {
                    bad.push(format!(
                        "  {}: f_{entry:08x}+{to:#x}, where it jumps, is not an instruction of the function",
                        s.name
                    ));
                    continue;
                }
                if jumps.contains(&pc) || before.contains(&pc) {
                    bad.push(format!(
                        "  {}: {here} has a jump or a before site already: one site before an instruction",
                        s.name
                    ));
                    continue;
                }
                if let Some(lost) = holders.iter().find(|&&h| lift.function(h).is_none_or(|g| !g.contains(target))) {
                    bad.push(format!(
                        "  {}: {here} is lifted in f_{lost:08x} too, which does not have f_{entry:08x}+{to:#x}, where it jumps",
                        s.name
                    ));
                    continue;
                }
                // (where it jumps is another function's entry in a holder: the lifted code would tail-call that function
                // there, not jump to the instruction)
                if let Some(h) = holders.iter().find(|&&h| target != h && lift.is_entry(target)) {
                    bad.push(format!(
                        "  {}: f_{entry:08x}+{to:#x}, where it jumps, is a function's entry: in f_{h:08x} it would be a \
                         tail call of it",
                        s.name
                    ));
                    continue;
                }
                jumps.insert(pc);
                for &h in &holders {
                    out.labels.entry(h).or_default().insert(target);
                }
                Point::Jump { pc, to: target }
            }
            Form::After => {
                if replaced.contains(&pc) {
                    bad.push(format!(
                        "  {}: {here} is replaced by another site, or has C after it (Lifter.after): one or the other",
                        s.name
                    ));
                    continue;
                }
                after.insert(pc);
                Point::After { pc }
            }
            Form::Before => {
                if jumps.contains(&pc) {
                    bad.push(format!("  {}: {here} has a jump site already: one site before an instruction", s.name));
                    continue;
                }
                before.insert(pc);
                Point::Before { pc }
            }
            Form::Branch | Form::Allow => {
                let (mine, theirs) =
                    if c.form == Form::Branch { (&mut branches, &allows) } else { (&mut allows, &branches) };
                if mine.contains(&pc) || theirs.contains(&pc) {
                    let (a, b) =
                        if c.form == Form::Branch { ("a branch", "an allow") } else { ("an allow", "a branch") };
                    bad.push(format!("  {}: {here} has {a} or {b} site already: one per branch", s.name));
                    continue;
                }
                mine.insert(pc);
                if c.form == Form::Branch { Point::Branch { pc } } else { Point::Allow { pc } }
            }
            _ => {
                if replaced.contains(&pc) || after.contains(&pc) {
                    bad.push(format!(
                        "  {}: {here} is replaced by another site, or has C after it (Lifter.after): one or the other",
                        s.name
                    ));
                    continue;
                }
                let callee = match kind {
                    InsnKind::Bl(t) => {
                        let callee = lift.callee(t);
                        if !lift.is_entry(callee) || lift.is_setjmp_owner(callee) {
                            bad.push(format!(
                                "  {}: {here} calls {callee:#x}, which is not a function the lift calls plainly (no entry, \
                                 or a setjmp owner whose call takes a host setjmp): a replace site is not for it",
                                s.name
                            ));
                            continue;
                        }
                        Some(callee)
                    }
                    _ => None,
                };
                replaced.insert(pc);
                Point::Replace { pc, callee }
            }
        };
        let looping = in_loop(&f, lift, pc) || (c.form == Form::Jump && c.to.unwrap_or(0) <= offset);
        // (a before site on the instruction after a call stands for code after the call only when nothing else reaches it)
        let after_call = c.form == Form::Before
            && pc >= 4
            && matches!(f.kind(pc - 4), Some(InsnKind::Bl(_) | InsnKind::Blr(_)))
            && holders.iter().any(|h| {
                lift.function(*h).is_some_and(|g| g.labels().contains(&pc))
                    || out.labels.get(h).is_some_and(|l| l.contains(&pc))
            });
        out.log.push(format!(
            "hooks: {} {} {here}{rule_note}{}{}{} [by {how}]",
            s.name,
            c.form.name(),
            if looping { " (in a loop: it runs each time round, measure it)" } else { "" },
            if after_call {
                " (WARNING: after a call, but a label: other paths reach it too, not only the call's return)"
            } else {
                ""
            },
            if holders.len() > 1 {
                format!(" (code {} other function(s) share: hooked there too)", holders.len() - 1)
            } else {
                String::new()
            },
        ));
        placed.holders = holders;
        out.sites.push(placed);
    }
    if !bad.is_empty() {
        return Err(ResolveError::DoNotFit(bad));
    }
    out.log.push(summary(sites, found));
    Ok(out)
}
