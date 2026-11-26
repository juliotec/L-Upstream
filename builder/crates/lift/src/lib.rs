//! The function builder: one function of a [`cfg::plan::Plan`] (the discovery's) and its instructions from the
//! [`image::Image`], as a [`mir::Function`].
//!
//! What it decides:
//! - a block starts at the entry, at every branch target inside the function, after every conditional branch, and
//!   where code resumes after an instruction that does not fall through;
//! - control reaching an address of the function that is no other function's entry (or is its own) goes to that
//!   block, polling first when it goes back (to an address at or below the branch's: a back edge);
//!   reaching another entry is a tail call of it (through any PLT stubs: the function they jump to); reaching
//!   anything else stops the guest there;
//! - BL and BLR write X30 first (BLR reads its target before), and call; RET returns; a BR with a recovered table is a
//!   switch over it, any other BR an indirect jump; SVC calls the host and goes on;
//! - an instruction that does not decode, that the semantics call undefined, or that they do not cover yet (the
//!   few FP16 forms left: [`sem::Fault::Unsupported`]), stops the guest there: a trap at its address, so a word
//!   discovery reaches by mistake never fails the build.
//!
//! **Edits** ([`edits::FunctionEdits`]: the hook sites, the game's patches, the entry hooks) are written in as the
//! function is built, where the builder knows each guest instruction's place: code before an instruction goes at the
//! start of its lowering (at its block's start when it starts one, so every way into it runs the code), code after one
//! after its lowering, a replaced instruction's lowering inside the code that keeps it, a branch site's code before the
//! branch's own test and an allow site's after it, the entry's code in a block of its own that only the call enters. A
//! function with a wrapper ([`edits::Wrap`]) is lifted as its body, under the wrapper's symbol's body name, and the
//! wrapper with it.

use a64::{Flow, Insn};
use cfg::plan::Plan;
use edits::{Edits, FunctionEdits, Target};
use mir::{BlockId, BlockOrigin, Dest, Edge, Function, Loc, Op, Pred, Term, Type};
use std::collections::{BTreeMap, BTreeSet};

/// Why a function is not lifted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The plan has no function at this entry.
    NoFunction,
    /// An edit that cannot go where it is (code after an instruction that ends its block).
    Edit {
        /// The instruction's address.
        pc: u64,
        /// What is wrong.
        what: &'static str,
    },
    /// A call of a setjmp owner with more setjmp calls than the host's slot holds ([`mir::SETJMP_RETS`]): a longjmp to
    /// one past the slot's would miss, so the lift stops rather than leave them out.
    SetjmpReturns {
        /// The owner's entry.
        owner: u64,
        /// Its setjmp calls.
        count: usize,
    },
}

impl Skip {
    /// The internal error of a setjmp owner with more setjmp calls than the host's slot holds.
    pub fn setjmp_returns_text(owner: u64, count: usize) -> String {
        format!(
            "the setjmp owner f_{owner:08x} has {count} setjmp calls, past the {} a host setjmp slot holds \
             (host/hos_setjmp.c SjSlot.rets, mir::SETJMP_RETS): an internal error of the lifter, report it",
            mir::SETJMP_RETS
        )
    }
}

/// The return addresses of a setjmp owner's calls as an [`Op::SetjmpCall`] carries them (the rest 0), or None when
/// there are more than [`mir::SETJMP_RETS`].
pub fn setjmp_rets(owned: &[u64]) -> Option<[u64; mir::SETJMP_RETS]> {
    let mut rets = [0u64; mir::SETJMP_RETS];
    rets.get_mut(..owned.len())?.copy_from_slice(owned);
    Some(rets)
}

/// A lifted function: its body, and the wrapper of its name when it has one (the body is then named for the wrapper
/// to call: [`edits::Wrap::body_symbol`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lifted {
    /// The function's code.
    pub body: Function,
    /// Its wrapper, `f_<entry>`, if it has one.
    pub wrapper: Option<Function>,
    /// The forms lowering does not do that its code has, each where it is (each a trap there: the lift's summary
    /// counts them, so a gap in coverage shows in the build's log before it shows as a guest's crash).
    pub unsupported: Vec<&'static str>,
}

impl Lifted {
    /// Its functions: the body, then the wrapper.
    pub fn functions(&self) -> impl Iterator<Item = &Function> {
        std::iter::once(&self.body).chain(&self.wrapper)
    }

    /// Its functions, for a pass to rewrite.
    pub fn functions_mut(&mut self) -> impl Iterator<Item = &mut Function> {
        std::iter::once(&mut self.body).chain(&mut self.wrapper)
    }
}

/// The program the builder lifts from: the plan's functions, the image's instructions, and the edits.
pub struct Program<'a> {
    image: &'a image::Image,
    plan: &'a Plan,
    edits: &'a Edits,
}

impl<'a> Program<'a> {
    /// The program of this image and plan, with these edits (none: [`Edits::default`]).
    pub fn new(image: &'a image::Image, plan: &'a Plan, edits: &'a Edits) -> Program<'a> {
        Program { image, plan, edits }
    }

    /// The function PLT stubs lead to from `t` ([`cfg::plan::Plan::callee`]).
    pub fn callee(&self, t: u64) -> u64 {
        self.plan.callee(t)
    }

    /// The function at `entry` as mid IR, with its edits, or why it is not lifted.
    pub fn lift(&self, entry: u64) -> Result<Lifted, Skip> {
        let func = self.plan.functions.get(&entry).ok_or(Skip::NoFunction)?;
        if let Some(stub) = self.plan.plt.get(&entry) {
            return Ok(Lifted { body: self.plt_stub(entry, stub), wrapper: None, unsupported: Vec::new() });
        }
        let edits = self.edits.get(&entry);
        let (mut body, unsupported) = Builder::new(self, entry, func, edits).build()?;
        let wrapper = edits.and_then(|e| e.wrap.as_ref()).map(|w| {
            body.set_symbol(edits::Wrap::body_symbol(entry));
            w.wrapper(entry)
        });
        Ok(Lifted { body, wrapper, unsupported })
    }

    /// Everything [`Program::lift`] reads to build the function at `entry`, written out: its instructions' addresses
    /// and words, its labels, its jump tables and devirtualised calls, and of each address its code or its edits' code
    /// may go to (a branch's, a call's, a table's target, the next instruction, an edit's goto or label) whether that
    /// is an entry, the function a call there
    /// reaches through the PLT, and that function's setjmp return addresses; a PLT stub's record. Two functions with
    /// the same text lift to the same IR, so a function's key is this text's hash, and a change elsewhere in the
    /// program (another function's code, an entry far away) leaves it as it was.
    pub fn inputs(&self, entry: u64) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let Some(func) = self.plan.functions.get(&entry) else { return s };
        let _ = write!(s, "{entry:x} labels={:?} plt={:?}", func.labels, {
            self.plan.plt.get(&entry).map(|p| (p.target, p.slot, self.callee(p.target)))
        });
        let mut targets = BTreeSet::new();
        for pc in func.pcs() {
            let w = self.image.insn(pc).unwrap_or(0);
            let _ = write!(s, " {pc:x}:{w:08x}");
            targets.extend([pc, pc + 4]);
            match a64::decode(w).map(|i| i.flow()) {
                Some(Flow::Jump { offset } | Flow::CondJump { offset } | Flow::Call { offset }) => {
                    targets.insert(pc.wrapping_add(offset as u64));
                }
                Some(Flow::JumpReg) => {
                    if let Some(t) = self.plan.jump_tables.get(&pc) {
                        let _ = write!(s, " jt={t:x?}");
                        targets.extend(t.iter().copied());
                    }
                }
                Some(Flow::CallReg) => {
                    if let Some(&u) = self.plan.devirt.get(&pc) {
                        let _ = write!(s, " devirt={u:x}");
                        targets.insert(u);
                    }
                }
                _ => {}
            }
        }
        // (and where its edits' code goes, and its labels: an entry there is a tail call)
        if let Some(e) = self.edits.get(&entry) {
            targets.extend(e.targets());
        }
        for t in targets {
            let entry = self.plan.entries.contains(&t);
            let callee = self.callee(t);
            if entry || callee != t {
                let owned = self.plan.setjmp_owners.get(&callee);
                let called = u8::from(self.plan.entries.contains(&callee));
                let _ = write!(s, " ->{t:x}:{}:{callee:x}:{called}:{owned:x?}", u8::from(entry));
            }
        }
        s
    }

    /// A PLT stub reached indirectly: X16 its GOT slot, X17 its target, as the stub leaves them, then its target.
    fn plt_stub(&self, entry: u64, stub: &cfg::plan::Plt) -> Function {
        let mut f = Function::new(entry);
        let b = f.start;
        let slot = f.push(b, Op::Const { ty: Type::I64, value: u128::from(stub.slot) });
        f.push(b, Op::Set(Loc::X(16), slot));
        let target = f.push(b, Op::Const { ty: Type::I64, value: u128::from(stub.target) });
        f.push(b, Op::Set(Loc::X(17), target));
        f.terminate(b, Term::Jump(Edge::new(Dest::Tail(self.callee(stub.target)), false)));
        f
    }
}

/// One instruction that falls through, as a function of its own at `pc` that runs it and returns: what the semantics
/// checks run against an emulator. `None` for a branch or a trap, or an instruction the semantics do not cover or call
/// undefined.
pub fn single(word: u32, pc: u64) -> Option<Function> {
    let insn = a64::decode(word)?;
    if insn.flow() != Flow::Next {
        return None;
    }
    let mut f = Function::new(pc);
    let b = f.start;
    sem::lower(&mut f, b, pc, &insn).ok()?;
    f.terminate(b, Term::Return);
    Some(f)
}

/// Instructions that fall through, one after another from `pc`, as one function that runs them all and returns: the
/// semantics checks' sequences, where one instruction's result is the next one's input. `None` if any is a branch or a
/// trap, or one the semantics do not cover or call undefined.
pub fn straight(words: &[u32], pc: u64) -> Option<Function> {
    let mut f = Function::new(pc);
    let b = f.start;
    for (k, &word) in words.iter().enumerate() {
        let insn = a64::decode(word)?;
        if insn.flow() != Flow::Next {
            return None;
        }
        sem::lower(&mut f, b, pc + 4 * k as u64, &insn).ok()?;
    }
    f.terminate(b, Term::Return);
    Some(f)
}

/// No edits: a function no change touches.
static NO_EDITS: std::sync::LazyLock<FunctionEdits> = std::sync::LazyLock::new(FunctionEdits::default);

struct Builder<'p, 'a> {
    program: &'p Program<'a>,
    entry: u64,
    pcs: Vec<u64>,
    body: BTreeSet<u64>,
    f: Function,
    blocks: BTreeMap<u64, BlockId>,
    edits: &'a FunctionEdits,
    /// The guest instruction being lowered, and the start of the guest block it is in (edits' code is of it).
    at: u64,
    guest_block: u64,
    /// The forms lowering does not do met so far ([`Lifted::unsupported`]).
    unsupported: Vec<&'static str>,
}

impl<'p, 'a> Builder<'p, 'a> {
    fn new(
        program: &'p Program<'a>,
        entry: u64,
        func: &cfg::plan::Function,
        edits: Option<&'a FunctionEdits>,
    ) -> Builder<'p, 'a> {
        let pcs: Vec<u64> = func.pcs().collect();
        let body = pcs.iter().copied().collect();
        Builder {
            program,
            entry,
            pcs,
            body,
            f: Function::new(entry),
            blocks: BTreeMap::new(),
            edits: edits.unwrap_or(&NO_EDITS),
            at: entry,
            guest_block: entry,
            unsupported: Vec::new(),
        }
    }

    fn word(&self, pc: u64) -> u32 {
        // (a discovered instruction is in the image's code: the discovery read it from there)
        self.program.image.insn(pc).unwrap_or(0)
    }

    /// Where control goes inside the function from each instruction (branch targets, the next one after a branch).
    fn starts(&self) -> BTreeSet<u64> {
        let mut starts = BTreeSet::from([self.entry]);
        let mut falls = true;
        for &pc in &self.pcs {
            if !falls || !self.body.contains(&pc.wrapping_sub(4)) {
                starts.insert(pc);
            }
            let flow = a64::decode(self.word(pc)).map_or(Flow::Trap, |i| i.flow());
            falls = !matches!(flow, Flow::Jump { .. } | Flow::JumpReg | Flow::Return | Flow::Trap);
            match flow {
                Flow::Jump { offset } => {
                    starts.insert(pc.wrapping_add(offset as u64));
                }
                Flow::CondJump { offset } => {
                    starts.insert(pc.wrapping_add(offset as u64));
                    starts.insert(pc + 4);
                }
                Flow::JumpReg => {
                    if let Some(targets) = self.program.plan.jump_tables.get(&pc) {
                        starts.extend(targets.iter().copied());
                    }
                }
                _ => {}
            }
        }
        starts.extend(self.edits.labels.iter().copied());
        starts.retain(|pc| self.body.contains(pc));
        starts
    }

    /// The edge for control reaching `t` from the instruction at `from`.
    fn transfer(&self, from: u64, t: u64) -> Edge {
        let entries = &self.program.plan.entries;
        if self.body.contains(&t) && (!entries.contains(&t) || t == self.entry) {
            let b = self.blocks[&t];
            Edge::new(Dest::Block(b), t <= from)
        } else if entries.contains(&t) {
            Edge::new(Dest::Tail(self.program.callee(t)), false)
        } else {
            Edge::new(Dest::BadTarget(t), false)
        }
    }

    fn build(mut self) -> Result<(Function, Vec<&'static str>), Skip> {
        let starts = self.starts();
        for &pc in &starts {
            let b = if pc == self.entry { self.f.start } else { self.f.add_block(pc) };
            self.blocks.insert(pc, b);
        }
        let mut cur: Option<BlockId> = None;
        for i in 0..self.pcs.len() {
            let pc = self.pcs[i];
            if let Some(&b) = self.blocks.get(&pc) {
                if let Some(prev) = cur
                    && !self.f.is_terminated(prev)
                {
                    let e = self.transfer(pc - 4, pc);
                    self.f.terminate(prev, Term::Jump(e));
                }
                cur = Some(b);
                self.guest_block = pc;
            }
            let Some(b) = cur else {
                continue; // (no block reaches this instruction from above, and nothing branches to it)
            };
            if self.f.is_terminated(b) {
                continue;
            }
            // (the block the instruction's code ends in, after any edits around it: the code after it goes on there)
            let b = self.edited(b, pc)?;
            cur = Some(b);
            let next_is_ours = self.pcs.get(i + 1) == Some(&(pc + 4));
            if !self.f.is_terminated(b) && !next_is_ours {
                let e = self.transfer(pc, pc + 4);
                self.f.terminate(b, Term::Jump(e));
            }
        }
        if let Some(b) = cur
            && !self.f.is_terminated(b)
        {
            let last = *self.pcs.last().expect("a function has an instruction");
            let e = self.transfer(last, last + 4);
            self.f.terminate(b, Term::Jump(e));
        }
        if !self.edits.entry.is_empty() {
            // (the entry's code in a block of its own: a branch back to the entry does not run it again)
            let start = self.f.start;
            let head = self.f.add_block_of(self.entry, BlockOrigin::Edit);
            self.guest_block = self.entry;
            let code = &self.edits.entry;
            if let Some(end) = edits::emit(&mut self, head, code)? {
                self.f.terminate(end, Term::Jump(Edge::new(Dest::Block(start), false)));
            }
            self.f.start = head;
        }
        Ok((self.f, self.unsupported))
    }

    /// The instruction at `pc` lowered into `b`, with its edits: the block control goes on in after it (a terminated
    /// one when every way through it ends).
    fn edited(&mut self, mut b: BlockId, pc: u64) -> Result<BlockId, Skip> {
        self.at = pc;
        let ed = self.edits;
        for code in [ed.before.get(&pc), ed.branch.get(&pc)].into_iter().flatten() {
            match edits::emit(self, b, code)? {
                Some(next) => b = next,
                None => return Ok(b),
            }
        }
        if let Some(code) = ed.replace.get(&pc) {
            match edits::emit(self, b, code)? {
                Some(next) => b = next,
                None => return Ok(b),
            }
        } else {
            self.instruction(b, pc)?;
        }
        if let Some(code) = ed.after.get(&pc) {
            // (code after an instruction that ends its block (a branch, a return, a trap) would never run: the
            // placement refuses such sites, and an edit that has one anyway stops the lift)
            if self.f.is_terminated(b) {
                return Err(Skip::Edit { pc, what: "code after an instruction that ends its block" });
            }
            match edits::emit(self, b, code)? {
                Some(next) => b = next,
                None => return Ok(b),
            }
        }
        Ok(b)
    }

    fn instruction(&mut self, b: BlockId, pc: u64) -> Result<(), Skip> {
        let word = self.word(pc);
        let Some(insn) = a64::decode(word) else {
            self.f.terminate(b, Term::Trap { pc, word });
            return Ok(());
        };
        let entries = &self.program.plan.entries;
        match insn.flow() {
            Flow::Next => match sem::lower(&mut self.f, b, pc, &insn) {
                Ok(()) => {}
                Err(sem::Fault::Undefined) => self.f.terminate(b, Term::Trap { pc, word }),
                Err(sem::Fault::Unsupported(form)) => {
                    self.unsupported.push(form);
                    self.f.terminate(b, Term::Trap { pc, word });
                }
            },
            Flow::Jump { offset } => {
                let e = self.transfer(pc, pc.wrapping_add(offset as u64));
                self.f.terminate(b, Term::Jump(e));
            }
            Flow::CondJump { offset } => {
                let cond = self.condition(b, pc, &insn);
                let then = self.transfer(pc, pc.wrapping_add(offset as u64));
                let otherwise = self.transfer(pc, pc + 4);
                match self.edits.allow.get(&pc) {
                    // (an allow site: its own test first, then the site's, asked only when the test holds)
                    Some(allow) => {
                        let check = self.f.add_block_of(self.guest_block, BlockOrigin::Edit);
                        let to_check = Edge::new(Dest::Block(check), false);
                        self.f.terminate(b, Term::Branch { cond, then: to_check, otherwise });
                        edits::emit_cond(self, check, allow, then, otherwise);
                    }
                    None => self.f.terminate(b, Term::Branch { cond, then, otherwise }),
                }
            }
            Flow::Call { offset } => {
                let t = self.program.callee(pc.wrapping_add(offset as u64));
                if entries.contains(&t) {
                    self.set_link(b, pc);
                    match self.program.plan.setjmp_owners.get(&t) {
                        Some(owned) => {
                            let rets =
                                setjmp_rets(owned).ok_or(Skip::SetjmpReturns { owner: t, count: owned.len() })?;
                            self.f.push(b, Op::SetjmpCall { target: t, rets })
                        }
                        None => self.f.push(b, Op::Call { target: t }),
                    };
                } else {
                    self.f.terminate(b, Term::Trap { pc, word });
                }
            }
            Flow::CallReg => {
                let Insn::BranchReg { rn, .. } = insn else { unreachable!("a BLR") };
                let target = sem::Sem::new(&mut self.f, b, pc).x(rn);
                self.set_link(b, pc);
                // (the register is compared with the usual address as the guest holds it, a PLT stub's or a
                // function's; the call goes to the function it leads to)
                let usual = self.program.plan.devirt.get(&pc).copied().and_then(|address| {
                    let callee = self.program.callee(address);
                    entries.contains(&callee).then_some(mir::Usual { address, callee })
                });
                self.f.push(b, Op::CallIndirect { target, usual });
            }
            Flow::JumpReg => {
                let Insn::BranchReg { rn, .. } = insn else { unreachable!("a BR") };
                let value = sem::Sem::new(&mut self.f, b, pc).x(rn);
                let term = match self.program.plan.jump_tables.get(&pc) {
                    Some(targets) => {
                        let distinct: BTreeSet<u64> = targets.iter().copied().collect();
                        let cases = distinct.into_iter().map(|t| (t, self.transfer(pc, t))).collect();
                        Term::Switch { value, cases }
                    }
                    None => Term::IndirectJump(value),
                };
                self.f.terminate(b, term);
            }
            Flow::Return => self.f.terminate(b, Term::Return),
            Flow::Svc => {
                let Insn::Svc { imm } = insn else { unreachable!("an SVC") };
                self.f.push(b, Op::Svc { imm });
            }
            Flow::Trap => self.f.terminate(b, Term::Trap { pc, word }),
        }
        Ok(())
    }

    fn set_link(&mut self, b: BlockId, pc: u64) {
        let ret = self.f.push(b, Op::Const { ty: Type::I64, value: u128::from(pc + 4) });
        self.f.push(b, Op::Set(Loc::X(30), ret));
    }

    /// A conditional branch's condition: B.cond's, CBZ's (the register against zero), TBZ's (one bit).
    fn condition(&mut self, b: BlockId, pc: u64, insn: &Insn) -> mir::Value {
        let mut s = sem::Sem::new(&mut self.f, b, pc);
        match *insn {
            Insn::BCond { cond, .. } => s.cond(cond),
            Insn::Cbz { sf, nonzero, rt, .. } => {
                let x = s.read(rt, sf, false);
                let zero = s.konst(if sf { Type::I64 } else { Type::I32 }, 0);
                s.cmp(if nonzero { Pred::Ne } else { Pred::Eq }, x, zero)
            }
            Insn::Tbz { nonzero, rt, bit, .. } => {
                let x = s.x(rt);
                let mask = s.k64(1u64 << bit);
                let tested = s.bin(mir::BinOp::And, x, mask);
                let zero = s.k64(0);
                s.cmp(if nonzero { Pred::Ne } else { Pred::Eq }, tested, zero)
            }
            _ => unreachable!("a conditional branch"),
        }
    }
}

impl edits::Emit for Builder<'_, '_> {
    type Error = Skip;

    fn func(&mut self) -> &mut Function {
        &mut self.f
    }

    fn guest_block(&self) -> u64 {
        self.guest_block
    }

    fn edge(&mut self, to: Target) -> Edge {
        let pc = self.at;
        let t = match to {
            Target::Pc(t) => t,
            Target::Taken => match a64::decode(self.word(pc)).map(|i| i.flow()) {
                Some(Flow::CondJump { offset }) => pc.wrapping_add(offset as u64),
                // (plan_edits puts a branch site's code at a conditional branch alone)
                _ => unreachable!("a branch site at {pc:#x}, no conditional branch"),
            },
        };
        self.transfer(pc, t)
    }

    fn insn(&mut self, b: BlockId) -> Result<(), Skip> {
        self.instruction(b, self.at)
    }
}

pub mod whole;

#[cfg(test)]
mod tests;
