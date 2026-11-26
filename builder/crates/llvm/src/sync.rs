//! Which locations the writer moves between their slots and the context, and where.
//!
//! The guest state lives in the context (`Ctx`) between functions, and in a slot per location (an `alloca`, which LLVM
//! makes a register) inside one. The two are synced where the state leaves the function or goes to the host (a call,
//! an SVC, a return, a tail call, a trap, an indirect jump), and where it comes back (the entry, after a call or SVC).
//! Syncing every location at each such point is correct but costs a load or a store per location, at every call. Two
//! dataflow passes find the ones that matter:
//!
//! - **dirty** (forwards, may): the slots that may hold a value the context does not, written since the last sync.
//!   Only those are spilled; a call leaves none dirty (it spilled them, and its own writes are in the context).
//! - **read** (backwards, may): the slots whose value is read before they are written: by a [`Op::Get`], or by a
//!   later spill (a dirty slot). Only those are reloaded, at the entry (of a split function's piece: what any of its
//!   entry blocks reads) and after a call.
//!
//! A fast path's helper ([`Op::Checked`]) is a pure function of its arguments, not a call of guest code: it neither
//! exposes nor changes the state, so it syncs nothing.
//!
//! A spill reads only slots that are dirty, and a dirty slot was written, or reloaded because a later spill reads
//! it: no spill reads a slot that holds nothing.
//!
//! A guest call (BL, BLR) keeps the calling convention's callee-saved registers, X19 to X29 and SP (after a call only
//! X0 to X18 and X30 are reloaded): their slots still hold their values after the call,
//! so they are not reloaded; one that is read after the call must hold its value before it instead. An SVC keeps
//! nothing (the host may write any register), nor does a host call with the context (a hook's callback, an entry
//! hook). `Options::callee_saved` off syncs every location at a call.

use mir::{BlockId, Function, Loc, Op, Value};
use std::collections::HashMap;

/// What is spilled before and reloaded after each call or SVC.
pub(crate) type Around = HashMap<Value, (Vec<Loc>, Vec<Loc>)>;

/// Where the writer syncs which locations.
pub(crate) struct Sync {
    /// Reloaded at the entry.
    pub entry: Vec<Loc>,
    /// Spilled before each call or SVC, and reloaded after it.
    pub around: Around,
    /// Spilled where each block's state leaves the function (its exits and its edges' tail calls and traps).
    pub exit: Vec<Vec<Loc>>,
}

pub(crate) fn sync(f: &Function, callee_saved: bool) -> Sync {
    let nb = f.block_count();
    let block = |b: usize| f.block(BlockId(b as u32));
    let term = |b: usize| block(b).term.as_ref().expect("a verified function");
    let succs = f.succs();
    let mut preds = vec![Vec::new(); nb];
    for (b, ss) in succs.iter().enumerate() {
        for &s in ss {
            preds[s].push(b);
        }
    }
    let exits: Vec<bool> = (0..nb).map(|b| term(b).exposes_state()).collect();
    let start = f.start.0 as usize;

    // dirty, forwards
    // (one instruction's effect on the dirty set: after a call none is, the callee wrote the context)
    let dirty_step = |v: Value, d: u128| -> u128 {
        match f.inst(v).op {
            Op::Set(loc, _) => d | loc.bit(),
            ref op if op.exposes_state() => 0,
            _ => d,
        }
    };
    let dirty_through = |b: usize, d: u128| -> u128 { block(b).insts.iter().fold(d, |d, &v| dirty_step(v, d)) };
    let mut dirty_in = vec![0u128; nb];
    let mut dirty_out = vec![0u128; nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in 0..nb {
            // (the entry adds nothing dirty: its slots were just reloaded; a loop back to it does)
            let din = preds[b].iter().fold(0, |acc, &p| acc | dirty_out[p]);
            let dout = dirty_through(b, din);
            if din != dirty_in[b] || dout != dirty_out[b] {
                dirty_in[b] = din;
                dirty_out[b] = dout;
                changed = true;
            }
        }
    }

    // read, backwards
    let read_out = |b: usize, read_in: &[u128]| -> u128 {
        let mut r = succs[b].iter().fold(0, |acc, &s| acc | read_in[s]);
        if exits[b] {
            r |= dirty_out[b];
        }
        r
    };
    // (the dirty set before each instruction of a block, for the calls' spills)
    let dirty_before = |b: usize| -> Vec<u128> {
        let mut d = dirty_in[b];
        let mut out = Vec::with_capacity(block(b).insts.len());
        for &v in &block(b).insts {
            out.push(d);
            d = dirty_step(v, d);
        }
        out
    };
    let dirty_at: Vec<Vec<u128>> = (0..nb).map(dirty_before).collect();
    // (what a guest call gives back as it was)
    let kept = if callee_saved { (19..=29).map(|n| Loc::X(n).bit()).fold(Loc::Sp.bit(), |acc, b| acc | b) } else { 0 };
    let read_through = |b: usize, mut r: u128, around: Option<&mut Around>| -> u128 {
        let mut around = around;
        for (i, &v) in block(b).insts.iter().enumerate().rev() {
            match f.inst(v).op {
                Op::Get(loc) => r |= loc.bit(),
                Op::Set(loc, _) => r &= !loc.bit(),
                ref op if op.exposes_state() => {
                    // (a call reads everything and may write everything but what it keeps: the dirty slots spilled
                    // before it, the slots read after it reloaded)
                    let keeps = if op.keeps_callee_saved() { kept } else { 0 };
                    let spill = dirty_at[b][i];
                    if let Some(a) = around.as_deref_mut() {
                        a.insert(v, (Loc::from_bits(spill), Loc::from_bits(r & !keeps)));
                    }
                    r = spill | (r & keeps);
                }
                _ => {}
            }
        }
        r
    };
    let mut read_in = vec![0u128; nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..nb).rev() {
            let r = read_through(b, read_out(b, &read_in), None);
            if r != read_in[b] {
                read_in[b] = r;
                changed = true;
            }
        }
    }

    let mut around = HashMap::new();
    for b in 0..nb {
        read_through(b, read_out(b, &read_in), Some(&mut around));
    }
    // (a piece of a split function may be entered at any of its entries: what any of them reads)
    let entry = f.entries.iter().fold(read_in[start], |acc, b| acc | read_in[b.0 as usize]);
    Sync { entry: Loc::from_bits(entry), around, exit: (0..nb).map(|b| Loc::from_bits(dirty_out[b])).collect() }
}
