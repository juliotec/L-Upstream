//! The guest state's reads and writes, simplified: what LLVM would find too, done before it so that it has much less
//! IR to read (most of a lifted function is its registers' and flags' loads and stores).
//!
//! 1. [`forward`]: in a block, a location read after it was written or read, with no call or SVC between, is the
//!    value it has (a block's values are its own: a location's value reaches another block through the location).
//! 2. [`share_pure`]: in a block, a pure op that an earlier one already computes (the same op of the same operands,
//!    once reads are forwarded: each instruction's FPCR bits, a register's bitcasts) is that op's value.
//! 3. [`dead_writes`]: a write of a location that nothing reads before it is written again, on every path, goes
//!    (most flag writes: an ADDS whose flags no branch reads). A location is read by every way the state leaves the
//!    function or goes to the host: a call, an SVC, a return, a tail call, a trap, an indirect jump.
//! 4. [`dead_values`]: an instruction that only makes a value nobody reads goes.

use mir::{BlockId, Function, Loc, Op, Value};
use std::collections::HashMap;

/// What [`simplify`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SimplifyStats {
    /// Reads of a location replaced by the value it holds.
    pub forwarded: usize,
    /// Pure ops replaced by an earlier one's value.
    pub shared: usize,
    /// Writes of a location nothing read.
    pub dead_writes: usize,
    /// Instructions whose value nothing read.
    pub dead_values: usize,
}

impl std::ops::AddAssign for SimplifyStats {
    fn add_assign(&mut self, o: SimplifyStats) {
        self.forwarded += o.forwarded;
        self.shared += o.shared;
        self.dead_writes += o.dead_writes;
        self.dead_values += o.dead_values;
    }
}

/// `f` simplified ([`forward`], [`share_pure`], [`dead_writes`], [`dead_values`]); what went.
pub fn simplify(f: &mut Function) -> SimplifyStats {
    let forwarded = forward(f);
    let shared = share_pure(f);
    SimplifyStats { forwarded, shared, dead_writes: dead_writes(f), dead_values: dead_values(f) }
}

/// Each block's pure ops that an earlier pure op of the block computes already (the same op, of the same operands
/// once those are rewritten): their uses given that op's value. How many. (Their instructions stay, unused, for
/// [`dead_values`].)
pub fn share_pure(f: &mut Function) -> usize {
    let mut n = 0;
    for b in 0..f.block_count() {
        let b = BlockId(b as u32);
        let insts = f.block(b).insts.clone();
        let mut seen: HashMap<Op, Value> = HashMap::new();
        let mut subst: HashMap<Value, Value> = HashMap::new();
        for v in insts {
            for x in f.op_mut(v).operands_mut() {
                if let Some(&y) = subst.get(x) {
                    *x = y;
                }
            }
            let op = &f.inst(v).op;
            if !op.is_pure() {
                continue;
            }
            match seen.get(op) {
                Some(&earlier) => {
                    subst.insert(v, earlier);
                    n += 1;
                }
                None => {
                    seen.insert(op.clone(), v);
                }
            }
        }
        if let Some(x) = f.term_mut(b).and_then(|t| t.operand_mut())
            && let Some(&y) = subst.get(x)
        {
            *x = y;
        }
    }
    n
}

/// Each block's reads of a location that a write or a read before it in the block (and no call or SVC between)
/// already gives: the reads replaced by that value. How many.
pub fn forward(f: &mut Function) -> usize {
    let mut n = 0;
    for b in 0..f.block_count() {
        let b = BlockId(b as u32);
        let insts = f.block(b).insts.clone();
        let mut held: HashMap<Loc, Value> = HashMap::new();
        let mut subst: HashMap<Value, Value> = HashMap::new();
        let mut kept = Vec::with_capacity(insts.len());
        for v in insts {
            for x in f.op_mut(v).operands_mut() {
                if let Some(&y) = subst.get(x) {
                    *x = y;
                }
            }
            match *f.op_mut(v) {
                Op::Get(loc) => match held.get(&loc) {
                    Some(&known) => {
                        subst.insert(v, known);
                        n += 1;
                        continue;
                    }
                    None => {
                        held.insert(loc, v);
                    }
                },
                Op::Set(loc, x) => {
                    held.insert(loc, x);
                }
                ref op if op.exposes_state() => held.clear(),
                _ => {}
            }
            kept.push(v);
        }
        if let Some(x) = f.term_mut(b).and_then(|t| t.operand_mut())
            && let Some(&y) = subst.get(x)
        {
            *x = y;
        }
        f.set_insts(b, kept);
    }
    n
}

const ALL: u128 = u128::MAX;

/// The writes of a location that nothing reads before it is written again, removed. How many.
pub fn dead_writes(f: &mut Function) -> usize {
    let nb = f.block_count();
    let succs = f.succs();
    let exits: Vec<bool> =
        (0..nb).map(|b| f.block(BlockId(b as u32)).term.as_ref().is_some_and(|t| t.exposes_state())).collect();

    // a block's effect, backwards: what it reads before writing (gen) and what it writes first (kill), up to its first
    // call or SVC from the end, before which everything is read
    let live_before = |f: &Function, b: usize, mut live: u128| -> u128 {
        for &v in f.block(BlockId(b as u32)).insts.iter().rev() {
            match f.inst(v).op {
                Op::Set(loc, _) => live &= !loc.bit(),
                Op::Get(loc) => live |= loc.bit(),
                ref op if op.exposes_state() => live = ALL,
                _ => {}
            }
        }
        live
    };
    let mut live_in = vec![0u128; nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..nb).rev() {
            let out = if exits[b] { ALL } else { succs[b].iter().fold(0, |acc, &s| acc | live_in[s]) };
            let new = live_before(f, b, out);
            if new != live_in[b] {
                live_in[b] = new;
                changed = true;
            }
        }
    }

    let mut removed = 0;
    for b in 0..nb {
        let mut live = if exits[b] { ALL } else { succs[b].iter().fold(0, |acc, &s| acc | live_in[s]) };
        let insts = f.block(BlockId(b as u32)).insts.clone();
        let mut kept = Vec::with_capacity(insts.len());
        for &v in insts.iter().rev() {
            match f.inst(v).op {
                Op::Set(loc, _) => {
                    if live & loc.bit() == 0 {
                        removed += 1;
                        continue;
                    }
                    live &= !loc.bit();
                }
                Op::Get(loc) => live |= loc.bit(),
                ref op if op.exposes_state() => live = ALL,
                _ => {}
            }
            kept.push(v);
        }
        kept.reverse();
        f.set_insts(BlockId(b as u32), kept);
    }
    removed
}

/// The instructions that only make a value no one reads, removed. How many.
pub fn dead_values(f: &mut Function) -> usize {
    let mut removed = 0;
    for b in 0..f.block_count() {
        let b = BlockId(b as u32);
        let insts = f.block(b).insts.clone();
        let mut uses: HashMap<Value, usize> = HashMap::new();
        for &v in &insts {
            for x in f.inst(v).op.operands() {
                *uses.entry(x).or_default() += 1;
            }
        }
        if let Some(x) = f.term_mut(b).and_then(|t| t.operand_mut()) {
            *uses.entry(*x).or_default() += 1;
        }
        let mut kept = Vec::with_capacity(insts.len());
        for &v in insts.iter().rev() {
            if f.inst(v).op.is_removable() && uses.get(&v).copied().unwrap_or(0) == 0 {
                for x in f.inst(v).op.operands() {
                    if let Some(n) = uses.get_mut(&x) {
                        *n -= 1;
                    }
                }
                removed += 1;
                continue;
            }
            kept.push(v);
        }
        kept.reverse();
        f.set_insts(b, kept);
    }
    removed
}
