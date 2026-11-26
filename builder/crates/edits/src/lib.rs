//! What Longshot changes in the game's functions, as edits of the mid IR: every hook site of the tables
//! (docs/dev/reference/hooks.md), the supported build's own patches ([`patches`]), and the host's entry hooks of the
//! SDK's functions ([`entry_hooks`]). Nothing here is C text, and nothing matches lifted text: each change is data,
//! [`code::Stmt`]s at a place of a function, and the function builder (the `lift` crate) writes them into the
//! function's IR as it builds it, where it knows each guest instruction's place.
//!
//! [`plan_edits`] gathers them per function ([`FunctionEdits`]): the code at the entry, before and after instructions,
//! in place of one, at a conditional branch, the labels a `jump` site adds, and a [`Wrap`] when the function runs under
//! a wrapper of its name (a `return` site's, the gameplay HUD's). A site whose instruction is lifted in several
//! functions (code they share) is in each.

pub mod code;
pub mod entry_hooks;
pub mod patches;
pub mod site;

pub use code::{Arg, Cond, Emit, Expr, Stmt, Target, Wrap, emit, emit_cond};

use cfg::plan::Plan;
use image::Image;
use site::SiteCode;
use std::collections::{BTreeMap, BTreeSet};

/// Why the edits cannot be planned: the lift stops, naming it.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// A patch whose place is not found exactly once.
    #[error("patch {what:?} in f_{function:08x}: {why}")]
    Patch {
        /// What it does.
        what: &'static str,
        /// Its function.
        function: u64,
        /// What is wrong.
        why: String,
    },
    /// Two edits that cannot both hold at one place.
    #[error("f_{function:08x}: {why}")]
    Clash {
        /// The function.
        function: u64,
        /// What clashes.
        why: String,
    },
    /// The host's sources could not be read.
    #[error("the host's sources: {0}")]
    Io(#[from] std::io::Error),
}

/// Everything one function gets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionEdits {
    /// Code at the entry, once per call, before anything of the function's own (a branch back to the entry does not
    /// run it): in order, the patches', the entry hook's, the sites'.
    pub entry: Vec<Stmt>,
    /// Code before an instruction, on every way into it (a patch's, then the sites' in the table's order).
    pub before: BTreeMap<u64, Vec<Stmt>>,
    /// Code after a plain instruction (a patch's, then the sites').
    pub after: BTreeMap<u64, Vec<Stmt>>,
    /// Code in place of an instruction, [`Stmt::Insn`] where it keeps it.
    pub replace: BTreeMap<u64, Vec<Stmt>>,
    /// Code before a conditional branch's own test ([`Target::Taken`]: the branch taken).
    pub branch: BTreeMap<u64, Vec<Stmt>>,
    /// A conditional branch taken only when its own test holds and then this.
    pub allow: BTreeMap<u64, Cond>,
    /// Instructions that must start a block (where a `jump` goes).
    pub labels: BTreeSet<u64>,
    /// The wrapper it runs under, if any.
    pub wrap: Option<Wrap>,
    /// What changes it, a line each (site and patch names), for the lift's report.
    pub why: Vec<String>,
}

impl FunctionEdits {
    /// Its content's key: a SHA-1 of everything it holds, the same in every run, for a cache of the lifted
    /// function (with the function's own words and the lifter's version). A change to one site, patch or entry hook
    /// changes the keys of the functions it lands in alone, as each function's edits hold only what lands in it.
    pub fn key(&self) -> [u8; 20] {
        // (its Debug text: every part of it is a Vec or a BTreeMap, in a fixed order, of plain data)
        let mut h = sha1_smol::Sha1::new();
        h.update(format!("{self:?}").as_bytes());
        h.digest().bytes()
    }

    /// The addresses its code may go to (its gotos' targets) and its labels: whether each is an entry decides what
    /// the edge there is (a block of the function or a tail call), so the function's inputs hold that.
    pub fn targets(&self) -> BTreeSet<u64> {
        fn walk(stmts: &[Stmt], out: &mut BTreeSet<u64>) {
            for s in stmts {
                match s {
                    Stmt::Goto(Target::Pc(t)) => {
                        out.insert(*t);
                    }
                    Stmt::If(_, a, b) => {
                        walk(a, out);
                        walk(b, out);
                    }
                    _ => {}
                }
            }
        }
        let mut out = self.labels.clone();
        walk(&self.entry, &mut out);
        for code in [&self.before, &self.after, &self.replace, &self.branch] {
            for stmts in code.values() {
                walk(stmts, &mut out);
            }
        }
        if let Some(Wrap::Return(after)) = &self.wrap {
            walk(after, &mut out);
        }
        out
    }

    /// Whether it changes nothing.
    pub fn is_empty(&self) -> bool {
        *self == FunctionEdits::default()
    }
}

/// Every function's edits, by entry.
pub type Edits = BTreeMap<u64, FunctionEdits>;

/// What [`plan_edits`] reads.
pub struct Inputs<'a> {
    /// The image.
    pub image: &'a Image,
    /// The lift's functions.
    pub plan: &'a Plan,
    /// The placed sites (`sites::apply`'s), if the build has a table.
    pub placement: Option<&'a sites::Placement>,
    /// The host's entry hooks (`entry_hooks::read`).
    pub entry_hooks: &'a [entry_hooks::EntryHook],
}

/// What [`plan_edits`] found: the edits, and its notes (entry hooks left out, patches not for this build).
pub struct Planned {
    /// The edits, by function.
    pub edits: Edits,
    /// A line for each thing left out, and why.
    pub notes: Vec<String>,
}

/// Every function's edits, from the placed sites, the patches (for the build they are written for) and the entry
/// hooks. Two edits that cannot both hold, or a patch not found exactly once in the build it is for, stop it.
pub fn plan_edits(inputs: &Inputs<'_>) -> Result<Planned, EditError> {
    let Inputs { image, plan, placement, entry_hooks: hooks } = *inputs;
    let mut edits = Edits::new();
    let mut notes = Vec::new();
    let callee = |t: u64| plan.callee(t);
    let word = |pc: u64| image.insn(pc);

    // the patches first: their code goes ahead of the sites' at one place
    let build = image.module("main").map(|m| m.build_id.to_ascii_lowercase()).unwrap_or_default();
    if build.starts_with(patches::BUILD) {
        for p in patches::patches() {
            let f = plan.functions.get(&p.function).ok_or_else(|| EditError::Patch {
                what: p.what,
                function: p.function,
                why: "the lift has no such function".into(),
            })?;
            let pc = patches::locate(&p, f, &word, &callee)?;
            let holders = plan.holders(pc);
            if p.find.is_some() && holders != [p.function] {
                return Err(EditError::Patch {
                    what: p.what,
                    function: p.function,
                    why: format!(
                        "{pc:#x} is lifted in {} functions: the patch is written for its own alone",
                        holders.len()
                    ),
                });
            }
            let e = edits.entry(p.function).or_default();
            e.why.push(format!("patch: {}", p.what));
            match p.place {
                patches::Place::Entry => e.entry.extend(p.code),
                patches::Place::Before => e.before.entry(pc).or_default().extend(p.code),
                patches::Place::After => e.after.entry(pc).or_default().extend(p.code),
            }
        }
        for (entry, wrap) in patches::wrappers() {
            if !plan.functions.contains_key(&entry) {
                return Err(EditError::Clash { function: entry, why: "the lift has no such function to wrap".into() });
            }
            let e = edits.entry(entry).or_default();
            e.why.push("patch: the gameplay HUD's wrapper".into());
            e.wrap = Some(wrap);
        }
    } else {
        notes.push(format!("patches: none for build {build} (they are for {})", patches::BUILD));
    }

    let (found, left) = entry_hooks::place(hooks, image, plan);
    notes.extend(left.into_iter().map(|n| format!("entry hooks: left out: {n}")));
    for (entry, h) in found {
        let e = edits.entry(entry).or_default();
        e.why.push(format!("entry hook: {}", h.symbol));
        e.entry.extend(entry_hooks::code(&h));
    }

    let is_call = |pc: u64| {
        word(pc).and_then(a64::decode).is_some_and(|i| matches!(i.flow(), a64::Flow::Call { .. } | a64::Flow::CallReg))
    };
    for p in placement.map(|p| p.sites.as_slice()).unwrap_or_default() {
        let code = site::code(p, is_call);
        let why = format!("site: {}", p.name);
        // (entry and return: the site's own function; the forms inside: each function its instruction is lifted in)
        for h in p.functions() {
            let e = edits.entry(h).or_default();
            e.why.push(why.clone());
            let clash = |why: String| EditError::Clash { function: h, why: format!("{}: {why}", p.name) };
            match code.clone() {
                SiteCode::Entry(c) => e.entry.extend(c),
                SiteCode::Return(c) => match &mut e.wrap {
                    None => e.wrap = Some(Wrap::Return(c)),
                    Some(Wrap::Return(after)) => after.extend(c),
                    Some(Wrap::Host { .. }) => return Err(clash("a return site on a function the host wraps".into())),
                },
                SiteCode::After(pc, c) => e.after.entry(pc).or_default().extend(c),
                SiteCode::Before(pc, c) => e.before.entry(pc).or_default().extend(c),
                SiteCode::Replace(pc, c) => {
                    if e.replace.insert(pc, c).is_some() {
                        return Err(clash(format!("{pc:#x} is replaced twice")));
                    }
                }
                SiteCode::Branch(pc, c) => {
                    if e.branch.insert(pc, c).is_some() || e.allow.contains_key(&pc) {
                        return Err(clash(format!("{pc:#x} has two branch or allow sites")));
                    }
                }
                SiteCode::Allow(pc, c) => {
                    if e.allow.insert(pc, c).is_some() || e.branch.contains_key(&pc) {
                        return Err(clash(format!("{pc:#x} has two branch or allow sites")));
                    }
                }
            }
        }
    }
    for (h, labels) in placement.map(|p| &p.labels).into_iter().flatten() {
        edits.entry(*h).or_default().labels.extend(labels);
    }
    // (a replaced instruction has no code after it: the table's check says one or the other)
    for (&h, e) in &edits {
        if let Some(pc) = e.replace.keys().find(|pc| e.after.contains_key(pc)) {
            return Err(EditError::Clash { function: h, why: format!("{pc:#x} is replaced and has code after it") });
        }
        // (a PLT stub is lifted as its jump, not its instructions: an edit there would never run)
        if plan.plt.contains_key(&h) && !e.is_empty() {
            return Err(EditError::Clash {
                function: h,
                why: "a PLT stub: its code is not lifted, so no edit can go in it".into(),
            });
        }
    }
    Ok(Planned { edits, notes })
}

#[cfg(test)]
mod tests;
